# Diagram aggregates: values over a set or a window of photos

Status: **design** (2026-11-14). The analysis plan was accepted by the
owner on that date and amended the same evening (the latest word wins):
photographers' real use cases first, researched and cited, and
**windows** as first-class sets. The slices at the end wait for the
owner's go.

It builds on [diagram-items.md](diagram-items.md) (envelope, registers
with keep policies, settled and live entries, the broadcast rule, sync
tags; it sends whole-set conditions here), [extractors.md](extractors.md)
(extractor blocks on carriers, the per-image cache, the gather node, the
expression language of routers and filters) and
[workflow-level.md](../workflow-level.md) (runs, one journal row per item
and node, resumable runs).

## In short

- **Real needs group photos first, then pick or measure inside each
  group.** Almost every case found (burst culling, best N per moment,
  sports plays, brackets, deflicker, astro calibration, matching a scene)
  is "split the stream into sets, compute one value per set, judge each
  item against it". Very few need a value over the whole import.
- **Sets** are the whole input, a group key, or a **window**:
  tumbling (N items, T seconds), sliding (the last N, the ±N
  neighbours), **gap** (a new set when the time between shots exceeds
  G), and **bracket** (driven by EXIF sequence and exposure data, gap as
  fallback). Each kind has a stated completeness rule.
- **Evaluation:** **two-pass** by default: an **aggregate node** computes
  per-set values and publishes them as register entries keyed by set;
  per-item conditions read the settled entry through the broadcast rule.
  **Barrier** only for many-to-one nodes (HDR merge, focus stack, astro
  stack), which consume the whole set. **Incremental** values are for
  display only, never for a decision with side effects.
- **Ordering:** window membership is computed from **capture time**
  (with sub-second time and sequence numbers), never from queue order;
  for library runs it is a pure function of the input's metadata, known
  at submit. Late items (live inputs only) never change a settled value
  by themselves.
- **Where:** aggregate **nodes** are the truth and come first; the
  expression sugar (`rank(burst, cull.sharpness) <= 2`) compiles to an
  aggregate node, a register and a reader, and comes later.
- **Cost:** values come from the per-image cache, so an aggregate adds no
  model call; the state per set is a list of `(item, value)` pairs,
  small even for 20,000 photos; resume recomputes from the cache.

## 1. Use cases from photographers

What tools and photographers already do, and what it asks of a diagram.
"Set" is how the photos are grouped, "aggregate" what is computed per
set, "model" the evaluation model of section 4.

| # | Use case | What photographers do or ask for | Set | Aggregates | Model |
|---|---|---|---|---|---|
| 1 | **Burst culling:** keep the sharpest, eyes-open frame of each burst | Lightroom Classic's Auto-Stack by Capture Time, with a "Time Between Stacks" slider [1]; darktable's AutoGrouper script groups by a gap in seconds [2]; FilterPixel's DeepCull tags a "Best of Burst" [3]; Aftershoot picks the best of each group of duplicates and guarantees one per set [4] | gap window | argmax (`best`), `count`, at least one per set | two-pass per set |
| 2 | **Best N per moment** (weddings, events) | Narrative Select's Scenes group similar photos to "pick the best of each moment" [5]; Aftershoot keeps 1, 2 or 3 of 10 depending on a setting [6] | gap window, or a similarity group | `top(n)` or a share of `count`, `rank` | two-pass per set |
| 3 | **Duplicates within a time window** | Excire's duplicate finder has a mode for series whose capture times differ slightly, with a time slider [7] | gap window plus similarity | `first`, `distinct`, `count` | two-pass per set |
| 4 | **Sports: best frame per play** | vendors describe 10 to 15 frames per play, only a few sharp at the peak, grouped by accurate timestamps [8] | gap window | argmax, `top(n)` | two-pass per set |
| 5 | **Time-lapse deflicker** | LRTimelapse smooths a luminance curve with a slider [9]; FFmpeg's `deflicker` is a moving average over 2 to 129 frames, 5 by default [10]; the `deflicker` Python tool matches a rolling mean over 10 images [11] | sliding window ±N, ordered by time | `mean` or `median` of a luminance key, per item | two-pass, per-item window |
| 6 | **Holy grail ramping** (day to night) | LRTimelapse puts keyframes "for each change in ISO, Shutter-Speed or Aperture" and offsets them [12] | segments where the exposure settings stay constant (a change-point window), inside the whole sequence | `first`/`last` per segment, step offsets, then the deflicker of 5 | two-pass over the sequence |
| 7 | **HDR brackets** | a long Adobe request asks auto-stacking to use exposure bias, since time gaps fail when a long exposure exceeds the interval [13]; a darktable script groups brackets by drive-mode EXIF [14]; digiKam merges separate bracket sets and is asked to use sequence metadata [15] | bracket window (EXIF), gap fallback | `count` against the bracket size, `min`/`max` exposure | barrier into a merge node |
| 8 | **Focus stacks** | Sony and Fujifilm write sequence fields, but Nikon NEFs lack them for focus shift [16]; photographers stack by capture time with a few seconds between groups [17] | bracket window, gap fallback | `count`, `first` | barrier into a merge node |
| 9 | **Astro calibration and stacking** | Siril builds master bias, darks and flats from their own sequences, then rejects outliers by sigma clipping, Winsorized by default [18] | group key: frame type, camera, ISO, exposure, temperature, session | per frame: `median`, MAD or `stddev` of quality keys (stars, FWHM) to reject bad subs; per pixel: a stack node | two-pass for frame rejection, barrier for the stack |
| 10 | **Scene consistency** (white balance, exposure) | Lightroom Classic's Match Total Exposures matches a group to a selected photo [19]; white balance is synced from a reference [20] | gap window, or a key (scene, folder) | `median` of exposure and white balance, per-item delta | two-pass per set |
| 11 | **Outliers in a catalogue or a session** | (no tool found that exposes it; Siril's rejection rate check, "below 0.5%" [21], is the same idea for frames) | group key (camera, day, session) | `percentile`, `median` and MAD | two-pass per set |

**Findings.**

- **Gap windows** serve cases 1 to 4, 8 and 10. Their pain points in the
  tools above are concrete: whole-second timestamps tie inside a burst
  (Photo Mechanic users show the seconds to see breaks [22]), and the
  gap is measured between shot starts, so long exposures break it [13].
  Sub-second time and the camera's sequence number must come first.
- **"At least one per set"** is a product rule (Aftershoot [4],
  FilterPixel [3]): a top-N keeps N even when all frames are poor,
  unless an explicit threshold filter follows.
- **Sliding windows** are per item (each frame has its own window) and
  serve deflicker only; they need time order and edge handling.
- **Brackets and stacks** are many-to-one: the set is the input of a
  merge, not a value. They share the window definitions but are
  barrier nodes.
- **Whole-input aggregates** are rare in practice (an outlier report);
  they block every reader until the end of the input, so the diagram
  shows that cost.
- **Similarity groups** (Narrative's Scenes [5], Excire [7]) are sets
  defined by an extractor (an embedding and a clustering), not by time;
  they are an aggregate themselves (section 2, nice-to-have).

## 2. The aggregates

**Needed first** (each cited case uses it):

| Aggregate | Input key types | Result | Cases |
|---|---|---|---|
| `count` | any | integer | 2, 7, 8 |
| `min`, `max` | number, integer, time | the input type | 7, 1 |
| `best` (argmax / argmin) | number, integer | an item reference and its value | 1, 4 |
| `top(n)` / `rank` | number, integer | ordered item references; per item, its rank | 2, 4 |
| `first`, `last` | any, by capture time | the value, or an item reference | 3, 6, 8 |
| `mean` | number, integer | number | 5 |
| `median` | number, integer | number | 5, 9, 10 |
| `mad` (median absolute deviation), `stddev` | number | number | 9, 11 |
| `percentile(p)` | number, integer | number | 11 |
| `distinct` | label, text, integer | a set of values and its size | 3 |

Grouping `by(key)` is not an aggregate but the set definition (section
3). `rank` ties break by capture time, then by item id, so results are
deterministic. Items lacking the key are skipped and counted
(`missing`); an empty set yields no value, read like an empty register
(diagram-items.md section 4).

**Nice-to-have:** `sum`, `mode`, a histogram, a weighted moving average
(sunrise sequences [11]), a share-based `top` (`top(30%)`), approximate
quantiles for very large sets (section 6), and **similarity groups**
(cluster an embedding key into sets).

**Not an aggregate:** per-pixel stacking (median, sigma clipping of
pixels, HDR and focus merges) is image work in a many-to-one node; it
reuses the set definitions here but its maths belongs to its module.

## 3. Sets and windows

A set definition is part of the aggregate node (and of a register's
"per set" scope, which reads the same definitions). The time of an item
is its **capture time** with sub-second precision, ties broken by the
camera's sequence number, then file name. Several bodies with drifting
clocks need a per-camera offset (open question).

| Kind | Defined by | Complete when | Notes |
|---|---|---|---|
| **whole input** | the run's input | the end of the input: every item concluded or omitted at the node | blocks readers until the end |
| **group key** | a key: folder, camera, day, import, a key expression | the end of the input; earlier if the input is sorted by that key and the key changed | camera and day are read from the catalogue, no model |
| **tumbling, N items** | N consecutive items in time order | its Nth item, or the end of the input | a short last set is flagged `partial` |
| **tumbling, T seconds** | fixed time slots | an item at or after the slot's end, or the end of the input | |
| **sliding, last N / ±N** | for each item, its N previous or N on each side | item i's window when item i+N is there (±N), at once for "last N" | edge mode: `shrink` (default) or `mirror`; one window per item |
| **gap (session)** | a new set when the time to the next shot exceeds G | the next item beyond G, or the end of the input | gap measured from the previous shot's **end** (start + exposure time) when known, so long exposures do not split brackets |
| **bracket** | EXIF: drive or bracket mode, sequence number, bracket size, exposure bias steps | the declared bracket size reached, a new sequence start, or the gap fallback | per-maker support varies; falls back to gap with a warning |
| **change-point** | a new set when given EXIF keys change (ISO, shutter, aperture) | the next item with a change, or the end of the input | holy grail segments (case 6) |

### Completeness and order

- **Library runs** (the input is known at submit): membership is a pure
  function of the input's metadata, sorted by capture time. The run
  computes it **at submit**, from the catalogue (EXIF), before any
  extractor runs. A set is then complete when every member concluded
  or was omitted at the aggregate node: that is the journal's own
  watermark. Queue order and concurrency never change membership.
- **Live inputs** (an import in progress, a watched folder, tethering)
  get a **watermark** from the source: the capture time up to which it
  promises no more items (an import's sorted reading of a card), plus a
  **timeout** with no new item (a set-level setting, off for library
  runs). Gap and change-point windows close as soon as a later item is
  seen; with live inputs that is the watermark passing.
- **Late items** (a member arrives after its set settled): by default it
  is **recorded as late** and does not change the settled value; its own
  reading uses that value, marked "late" in the journal. Recomputing the
  set is an explicit retry, which makes a new register version (readers
  that already concluded keep the version they used). This mirrors
  stream processors' zero allowed lateness, where late events go to a
  side output [23].
- **Claim order:** the scheduler favours items of open sets in time
  order, so bursts settle one after another and readers start early,
  instead of every set closing at the end of the run.

## 4. Evaluation models, per use case

- **Barrier:** the node waits for the whole set, then runs once for it.
- **Two-pass:** an aggregate node computes per-set values once the set
  is complete and **publishes them as a register entry keyed by set**
  (diagram-items.md section 3: a register with a `per set` reset scope,
  settled on completion). Per-item conditions downstream read the
  settled entry through the broadcast rule; per-item values (a rank, a
  window mean) come back by resync with the item's sync tag. Items keep
  streaming between sets.
- **Incremental:** running values, provisional, revised as items come.

| Case | Model | Why |
|---|---|---|
| 1, 2, 3, 4 burst, moments, duplicates, plays | two-pass per set | sets are small and close quickly; the decision (pick, reject) has side effects, so it reads a settled value |
| 5 deflicker | two-pass, per-item sliding window | each item's target is settled once its N neighbours are in; no whole-sequence wait |
| 6 holy grail | two-pass over segments, then 5 | segment offsets need each segment closed |
| 7, 8 brackets, focus stacks | barrier into a many-to-one node | the merge needs every frame |
| 9 astro | two-pass to reject frames, barrier to stack | frame rejection reads per-group statistics; the stack consumes the kept frames |
| 10 scene consistency | two-pass per set | the median must be settled before edits are written |
| 11 outliers | two-pass, whole input or key | a report; the cost of waiting is shown |

**Recommendation:** two-pass with registers keyed by set is the model;
barrier is the many-to-one node's own nature, not a separate mechanism
(the gather node of extractors.md is a barrier with one output per
set); incremental values are shown live (the register's `live` read,
the Activity detail) but a node whose effect is a write, a pick or a
route reads **settled** entries only, and the diagram check enforces
it. No case found needed provisional decisions.

## 5. Where aggregates live, and type checking

- **Aggregate node (first):** inputs the item stream; declares the set
  (section 3), the extractor blocks it needs (or upstream keys), and one
  or more named outputs (`burst.best = best(cull.sharpness)`,
  `burst.n = count()`). Its outputs are register entries keyed by set,
  desynchronized, read like any register. Items pass through unchanged
  (it watches, like a register with an input). It is visible in the
  diagram, which is what makes a wait understandable.
- **Expression sugar (later):** a router or filter condition may say
  `rank(burst, cull.sharpness) <= 2` or
  `cull.exposure > median(scene, cull.exposure) + 1`. At the diagram
  check it compiles to an aggregate node, its register and a resync,
  shown on demand ("this filter waits for set `burst`"). Two uses of the
  same aggregate over the same set share one node. The sugar needs the
  set named (`burst`, `scene`), never an implicit "the set".
- **Type checking** at the diagram check, against declared keys:
  numeric aggregates refuse labels and text; `min`/`max` accept time;
  `best` and `top` return item references usable by resync; `distinct`
  refuses numbers with a unit mismatch; a set key must exist on every
  path; a decision node reading a `live` aggregate is refused. Errors
  name the node, the aggregate and the key.

## 6. Cost, caching, memory, resume

- **Values come from the per-image cache:** an aggregate's extractor
  blocks are the same uses as the carriers' (one cache entry per image,
  extractor version and model), so re-running a diagram or adding an
  aggregate costs no model call. Set membership costs only catalogue
  reads.
- **State per set:** `count`, `min`, `max`, `mean` are O(1); `median`,
  `percentile`, `mad`, `rank`, `top` keep the `(item, value)` pairs:
  about 40 bytes per item, so a 20,000-photo whole-input aggregate is
  under 1 MB per key. Exact quantiles are the rule; an approximate
  sketch is a nice-to-have only beyond a large bound (a million items).
  Sliding windows keep 2N+1 values.
- **Time:** sorting is O(n log n) per set; negligible against
  extractors. The real cost of an aggregate is **latency**: readers of
  a whole-input aggregate wait for the whole run, which the diagram and
  the Activity focus say.
- **Resume:** the state is not persisted. After a stop, the aggregate
  rebuilds open sets from the journal (which members concluded) and the
  cache (their values); settled sets are register versions already
  stored. A recomputed set that would differ (a changed extractor
  version) makes a new version, never a silent change.

## 7. UX

- **In the diagram:** an aggregate node shows its set ("per burst: gap
  0.5 s") and outputs; edges carrying set values are drawn as register
  edges (desynchronized). A node waiting for a set shows a badge with
  the open sets count; the expanded sugar shows the hidden node.
- **Set preview without a run:** choosing a window shows its grouping
  on the input (from metadata only, instant): number of sets, sizes,
  a strip per set. This is how a photographer tunes the gap G.
- **Activity focus:** per aggregate node, sets `settled / open / total`
  and the current set's progress ("burst 12: 7 of 9 measured"); waiting
  items say "waiting for set burst 12". Late items are listed.
- **Preview on a sample:** a sample takes **whole sets**, never items
  scattered across sets, so the best of a burst in the preview is the
  best of the full burst. Whole-input aggregates over a sample are
  labelled approximate ("median over 200 of 4,312").

## 8. Agents and MCP

- Aggregate nodes and set definitions are part of the diagram document;
  an agent writes them like any node.
- `diagrams.sets.preview {input, set}`: the grouping only, from
  metadata, no run (an agent tunes a gap or checks brackets).
- `diagrams.sets.list {run, node}` and `.get {run, node, set}`: members,
  state (open, settled, partial), values and their register versions,
  late items.
- Existing `registers.*` reads (diagram-items.md section 8) show the
  published entries; a recompute is a run retry.

## 9. Open questions (not decided)

- Per-camera clock offsets for multi-body events: a setting per camera
  and session, or derived from overlapping subjects.
- Which makers' bracket and sequence EXIF fields come first (Sony,
  Fujifilm, Canon, Nikon's gaps [16]), through exiftool tags.
- Similarity groups (embedding clustering) as a set kind: when, and with
  which embedding.
- Default timeout for live inputs, and whether a late item may reopen a
  set automatically.
- Share-based `top(30%)` and an approximate quantile sketch: needed by
  any real case?

## 10. Slices

Each slice is one branch, building and passing its tests; code checked
locally, finalised on the box. They follow diagram-items.md slices 3 and
4 (register core, registers in runs).

1. **Set and window definitions (pure).** Whole, key, tumbling, sliding,
   gap (from the previous shot's end), bracket, change-point; ordering
   by capture time, sub-second, sequence; membership from metadata;
   completeness rules. *Tests:* golden groupings on synthetic series
   (whole-second ties resolved by sequence, a long exposure that does
   not split a bracket, a bracket without sequence data falling back to
   gap, a short last tumbling set flagged `partial`).
2. **Aggregate functions (pure).** The "needed" list of section 2,
   missing values, empty sets, ties, signatures for type checking.
   *Tests:* property tests against a naive implementation; deterministic
   ties; type refusals named.
3. **Aggregate node in runs (two-pass).** Membership at submit, journal
   rows per set, claim on completion, outputs published as register
   entries keyed by set, readers on settled entries, resync of per-item
   values. *Tests:* journal scenarios on the memory store with a fake
   extractor: a reader waits until its set settles; a resume after a
   stop makes zero extractor calls and the same versions.
4. **Set-ordered claiming.** The scheduler favours open sets in time
   order. *Tests:* on 20 synthetic bursts, the first burst settles
   before the last item of the run is measured.
5. **Sliding windows.** Per-item windows, edge modes. *Tests:* a
   synthetic luminance series with flicker: the rolling target matches
   a reference; item i settles once item i+N is in.
6. **Live inputs and late items.** Source watermarks, timeout, late
   items recorded, explicit recompute as a new version. *Tests:* fake
   clock; a late item leaves the settled value unchanged and is listed.
7. **Diagram check rules.** Decision nodes refuse `live` aggregates,
   set keys checked on every path, typed outputs. *Tests:* check
   refusals named by node, aggregate and key.
8. **Expression sugar.** `rank`, `best`, `median`… in conditions,
   compiled to aggregate nodes; shared nodes for identical uses.
   Depends on the expression language of routers and filters.
   *Tests:* golden compilation; a filter with `rank(burst, …) <= 2`
   gives the same result as the explicit nodes.
9. **Diagram UX.** Aggregate node, set badge, set preview from metadata.
   *Tests:* kittest through accesskit: change the gap, the set count
   updates; `library_scale` unchanged.
10. **Activity focus.** Per-set progress, waiting reasons, late items;
    sampling by whole sets with the approximate label. *Tests:* kittest
    on a finished and an in-progress run.
11. **MCP verbs.** `diagrams.sets.preview`, `.list`, `.get`. *Tests:*
    protocol tests per verb; an agent tunes a gap with `preview`, then
    runs.
12. **Templates.** Best of each burst (at least one per set), best N per
    moment, deflicker target, brackets into stacks, outliers against
    the session median, with fake extractors. *Tests:* each template end
    to end through the node's batch endpoint, with deadlines.

Slices 1–3 give agents a working model; 5 and 6 widen it; 9–10 give it
to the photographer.

## Sources

1. Adobe, Lightroom Classic, group photos into stacks:
   https://helpx.adobe.com/lightroom-classic/help/auto-stack.html
2. darktable Lua scripts, AutoGrouper:
   https://docs.darktable.org/lua/stable/lua.scripts.manual/scripts/contrib/autogrouper
3. FilterPixel, DeepCull: https://app.filterpixel.com/deepcull
4. Aftershoot, culling release notes (October 2022):
   https://devtest.aftershoot.com/?p=47051
5. Narrative, using Scenes in Select:
   https://help.narrative.so/en/articles/8119179-using-scenes-in-select
6. Aftershoot, getting started with culling:
   https://support.aftershoot.com/en/articles/5223473-get-started-with-aftershoot-culling
7. Excire, duplicate finder:
   https://excire.com/en/tutorials/excire-search/duplikatefinder/
8. FilterPixel, sports burst-mode culling:
   https://filterpixel.com/blog/sports-photography-burst-mode-culling
9. LRTimelapse forum, deflickering issues:
   https://forum.lrtimelapse.com/Thread-deflickering-issues
10. FFmpeg filters, deflicker:
    https://ayosec.github.io/ffmpeg-filters-docs/7.1/Filters/Video/deflicker.html
11. `deflicker` on PyPI: https://pypi.org/project/deflicker/0.1.1/
12. LRTimelapse forum, deflickering and 2/3-star transitions:
    https://forum.lrtimelapse.com/Thread-solved-deflickering-and-2-3-transitions
13. Adobe community, better auto stacking for bracketing, HDR, focus
    stacking and panoramas:
    https://community.adobe.com/t5/lightroom-classic-ideas/p-better-auto-stacking-for-bracketing-hdr-focus-stacking-and-panoramas/idi-p/12250005/page/2
14. pixls.us, managing bracketed shots in darktable:
    https://discuss.pixls.us/t/managing-and-organizing-bracketed-shots-in-darktable/51599
15. KDE bug tracker, digiKam timelapse/burst grouping:
    https://mail-archive.com/kde-bugs-dist@kde.org/msg1179768.html
16. diglloyd, Nikon Zf/Z8 sequence number:
    https://diglloyd.com/blog/2024/20240208_1112-NikonZf-NikonZ8-SequenceNumber.html
17. DPReview forum, managing focus-bracketed files:
    https://www.dpreview.com/forums/post/66188002
18. Siril documentation, stacking:
    https://siril.readthedocs.io/en/1.2/preprocessing/stacking.html
19. Lightroom Killer Tips, matching exposure across images:
    https://lightroomkillertips.com/automatically-matching-exposure-across-multiple-images/
20. Adobe community, matching white balance in a batch to a photo:
    https://community.adobe.com/questions-675/is-it-possible-to-match-white-balance-in-a-batch-to-a-certain-photo-955971
21. pixls.us, pixel rejection levels when stacking:
    https://discuss.pixls.us/t/pixel-rejection-levels-when-stacking/20800
22. Camerabits, customizing the contact sheet:
    https://home.camerabits.com/when-customizing-the-contact-sheet-saves-the-day/
23. Apache Flink, windows (allowed lateness, side output):
    https://nightlies.apache.org/flink/flink-docs-master/docs/dev/datastream/operators/windows/
