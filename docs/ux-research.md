# UX research for the Helios GUI

Status: research notes (2026-10-11), to drive the first mockups. Complements
[NEEDS.md](NEEDS.md), which covers needs at the product level. Sources were
found by web search; items marked [?] could not be fully checked, and vendor
figures (culling and editing times) are not neutral.

## 1. What users say about darktable's interface and its equivalents

**Overlapping tools, more than too many tools.** "The real issue is not that
there are too many modules… the confusion arises when multiple modules
provide the same functionality and differ only in the implementation
details" ([pixls, 2026](https://discuss.pixls.us/t/on-module-proliferation/56435)).
Three tone mappers (filmic, sigmoid, AgX), of which only one should be used
([pixls](https://discuss.pixls.us/t/filmic-vs-sigmoid-vs-agx-some-thoughts/54716)).
Users suggest labelling modules "general purpose" vs "specific use case".

**Unclear state and grammar.** Module group buttons are filters that look
like tabs; the on/off icon is too small to read; left and right clicks are
used inconsistently; there is no top menu
([pixls, 2026](https://discuss.pixls.us/t/darktable-ui-work/56268)). Which
images an action applies to (hover vs selection) has surprised users "since
2009" (darktable issues #16850, #6025).

**Layout depends on the screen.** A second module panel splits users:
ultrawide screens want it, laptops do not; floating-then-docking panels are
wanted by some and "always fiddly" for others
([pixls, 2026](https://discuss.pixls.us/t/rfc-secondary-module-panel/56745)).
The second window exists only in the darkroom
([pixls](https://discuss.pixls.us/t/2nd-monitor-in-lighttable-view/32138)).
Capture One's freely defined tool tabs are its most praised layout feature
([Photography Life](https://photographylife.com/switching-from-lightroom-to-capture-one-pro));
DxO's detachable palettes draw complaints about scrolling and two-column
layouts ([DxO forum](https://forum.dxo.com/t/14896)).

**Culling is slow.** "Waiting one second for the newly selected image to
render is too slow for any quick work" (darktable issue #21942). Photo
Mechanic is fast because it shows embedded previews and has no catalog.
Applying an edit to a burst is "too heavy" (#16688). Comparing with the
camera JPEG is a long-standing request (#5778).

**Masks.** AI masks are expected (Lightroom's one-click sky is praised) but
fail often, so they must stay editable by hand; drawn masks are CPU-bound
and sluggish on large images
([pixls](https://discuss.pixls.us/t/understanding-the-bottleneck-with-drawn-masks/56503) [?]).

**Shortcuts.** Ansel's author argues the darktable shortcut system serves
"about 2% of users" and moves global features into "a global menu" rather
than "undocumented shortcuts"
([Ansel](https://github.com/aurelienpierreeng/ansel)). ART traded "a bit of
customization and control… for a simpler and easier to use interface".

**Node graphs.** In vkdt, links draw over nodes unpredictably and
drag-to-delete was rejected as brittle in favour of an "insert before node"
button; each GUI module already hides a subgraph of GPU kernels
([pixls dev diary](https://discuss.pixls.us/t/vkdt-dev-diary-pt2/28540) [?]).
Resolve's color nodes are harder to learn than layers but "often feel more
intuitive" once read; Blender's compositor is "daunting", eased by lazy
connect and the F3 operator search.

**Comparison.** Lightroom's before/after cycles split modes; users want a
chosen history step as "before", and a reference view to match looks
between two photos.

## 2. How people actually work

| workflow | volume, pressure | where time is lost | what the ideal tool does |
|---|---|---|---|
| wedding / event | 2,000-5,000 shots, gallery in 2-4 weeks | culling (3-8 h by hand, 30-60 min hybrid AI + human [vendor]), matching the look across cameras | AI cull reviewed in groups, a look propagated across the series, gallery from the project |
| press / sports | a few images every break (about 14 min at the Olympics) | captions, card moves, untested sending | select during ingest, AI captions confirmed by a human, one-key send |
| landscape / fine art | tens of frames, deep edits | round trips to Photoshop, hand-painted blend masks | merges as nodes, named reusable masks, versions side by side, soft proof |
| studio / e-commerce | hundreds of SKUs, client in the room | feedback by email, version confusion, sign-off | roles, comments anchored on a zone, approval state per image |
| hobbyist, NAS + laptop | 50k-300k images | merging laptop edits back, finding old images | offline check-out by project, reconciliation by content hash, semantic search |
| astro / timelapse | hundreds to thousands of frames | wizards that break, full re-runs | explicit graph, incremental re-run, keyframes as a graph feature |
| film scanning | 36 frames a roll | fiddly pickers, inconsistent rolls | roll-level analysis feeding per-frame tweaks, AI-assisted dust |

Sources: Aftershoot, Imagen, PhotoUp (wedding, vendor figures); Fstoppers
and Jeff Cable (Olympic deadlines); Camera Bits (Photo Mechanic at sports
events); Capture One (tethering, Live); darktable manual and pixls threads
(local copies, merging databases, negadoctor); Siril and LRTimelapse docs and
forums.

Three findings cut across all of them:

1. **People think in jobs** (a wedding, a match, roll 12): projects are the
   right first-class object (ROADMAP 2.4).
2. **The AI that works is hybrid**: it proposes, a human confirms in groups.
   Fully automatic culling is distrusted. This is the AI-assisted mode of
   decision nodes.
3. **Many workflows are graphs hidden behind wizards** (stacking, blends,
   roll analysis, keyframes): the diagram GUI shows what they really are.

## 3. Implications for the GUI

1. **Tools by intent.** Modules carry a role (general purpose or
   specialised) and one recommended tool per job, alternatives one step
   away. Workshops expose only the useful sliders.
2. **Readable state, one grammar.** Active/inactive and changed/default are
   obvious; one click grammar; a real menu; a fuzzy command palette over
   modules, widgets and actions. Shortcuts are optional bindings of menu
   actions, never the only way.
3. **Layout adapts to the screen.** Dockable, sticky panels and saved
   workspaces per project; a minimal image-first layout for laptops; every
   view usable on a second window.
4. **Culling is a first-class mode.** Embedded previews first, prefetch of
   the next images by the scheduler, group review of AI proposals, one
   explicit selection model, quick sync of an edit across a burst.
5. **Compare against anything.** Before/after against any version or
   history step, the camera JPEG, or a reference photo, with synced zoom.
6. **Graph and stack are two views of one network.** The darktable-like
   stack is the default; the graph is the expert view; both stay in sync.
   Modules are collapsible subgraphs; edits use buttons ("insert before")
   and lazy connect rather than precise gestures; the active node's edges
   are highlighted.
7. **AI proposals stay editable.** Masks, picks and captions from AI are
   always corrected by hand in the same place they are shown.

## 4. Scenarios for the mockups

Each scenario is a script to walk through a mockup; success criteria in
brackets.

- **S1. Wedding cull** (project, AI-assisted decision node). 3,200 raws
  from two bodies; a cull node groups bursts and near-duplicates and
  proposes picks with reasons; the photographer reviews group by group and
  overrides in batch. [Done in under 45 minutes; every override recorded.]
- **S2. One look across the series** (workshop). "Warm, soft, a bit matte;
  match camera B to camera A" opens a workshop of real modules with four
  sliders; applied to the project, scene-dependent frames go to a decision
  node. [Consistent series, short list of exceptions, workshop saved.]
- **S3. Deadline send** (multi-node, graph). Laptop at the stadium, editor
  remote; rating during ingest; AI captions from a roster confirmed by a
  human; one-key send. [Five captioned images out in under ten minutes;
  status of each send visible.]
- **S4. Landscape blend** (graph and stack views). Brackets and a panorama
  merged as nodes feeding the usual pipe; a sky-mask decision node offers
  three masks; switch to the stack view for fine work; soft proof. [The user
  sees where each input enters; switching views loses nothing.]
- **S5. Studio approval** (collaboration, roles). The client marks "label
  glare" on a zone; the thread is anchored to image, version and zone; the
  AI proposes a branch, the retoucher adjusts, the art director approves.
  [No email; one owner per image at each state.]
- **S6. Travel check-out** (vault, multi-node). A project and a saved
  search checked out to the laptop, edited offline, reconciled at home by
  hash. [Zero manual merging.]
- **S7. Astro stack** (graph, recipe). Lights, darks, flats and bias become
  a calibration and stack graph; a full-AI node rejects bad subframes with a
  report; adding frames re-runs only what changed. [Readable graph,
  incremental re-run.]
- **S8. Film roll** (graph, decision node, workshop). A roll-analysis node
  feeds per-frame nodes with local overrides; dust spots confirmed in batch.
  [Consistent roll in minutes; exceptions visible.]
