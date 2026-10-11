# Helios: checking the roadmap against real needs

State as of 2026-10-10. Sources: darktable GitHub issues and RFCs (via `gh`, read-only), the
discuss.pixls.us forum, competitors' product pages and forums, measurements on a development laptop.
GitHub reaction counts are low everywhere (the most upvoted issue in the project's history,
darktable-org/darktable#2170 "CR3 support", has 58 👍): demand is mostly read from the number of comments and from pixls threads.
Points marked [?] could not be verified.

## On one page

| Roadmap axis | Demand from darktable users | Maintainers' position | Competition | Verdict |
|---|---|---|---|---|
| Client/server catalog, NAS, multi-workstation | **moderate and recurring** (several pixls threads, darktable-org/darktable#5904, darktable-org/darktable#18253 with 55 comments) | **against** ("too big a change", "to be done as an add-on") | **real gap**: Lightroom Classic refuses a shared catalog, Capture One has just released a multi-user mode, **LAN, Mac first, beta** (June 2026) | **the most solid need**, but outside upstream |
| Self-describing modules, headless/cloud engine | low; already partly in master (MCP + introspection, darktable-org/darktable#21573) | in favor of introspection | cloud rendering: no consumer product [?] | **an architectural means, not an expressed need** |
| Parallel / distributed batches | **real pain** about slow export (darktable-org/darktable#17538, darktable-org/darktable#16207, darktable-org/darktable#16958) | against parallel export ("technically very difficult, more memory") | commonplace locally at every competitor | real need **on one machine**; nobody asks for multi-machine |
| AI-first, "intent -> workshop" | the complaint about the learning curve is **strong**; nobody asks to "describe an intent" | **against simplification** (darktable-org/darktable#19084 rejected, RFC darktable-org/darktable#22024 "not for someone who wants a one-click solution") | "AI learns your style" is **commoditized** (Imagen, Aftershoot, Lightroom) | **hypothesis to validate**, differentiating angle unproven |
| Collaboration, roles, anchored comments | **none** among darktable users | - | **real gap** on the studio side: client feedback poorly linked to the edits (Lightroom, Capture One Live) | real need **of a different audience** than darktable's |
| Plugins | low; Lua serves as the plugin API | send experiments to Lua | - | secondary |

## What this changes for the roadmap

1. **The target audience decides everything.** Current darktable users (mostly amateurs,
   self-hosted) ask for multi-workstation and speed, not for collaboration or AI-intent.
   Studios ask for a shared catalog and client approval, but pay for Capture One and expect
   support, recent camera bodies, stability. The open question "solo photographer or small studios"
   (Helios ROADMAP, open questions) is no longer secondary: it is the first decision.
2. **The shared catalog is the best-supported need**, and phase 0 (catalog SDK) is its
   technical prerequisite. But upstream does not want it: phase 0 will only be accepted upstream if
   presented as a cleanup (less SQL in the GUI), never as a step toward client/server.
3. **Parallel export is won first on a single machine.** See the measurements below. Distributed
   will come if local is already saturated, which nobody reports.
4. **The "intent -> workshop" AI must be tested before it is built**: mockup, a few
   photographers, measurement of the time saved. The "AI that retouches for you" field is already taken and
   relies on training data that Helios does not have.
5. **Self-describing modules remain justified as architecture** (headless, scheduling,
   generated workshops), but do not sell on their own: to be pursued in service of a need above.

## The platform argument

The user's position (2026-10-10), in their words: "a better-written framework lets you move faster on all
fronts". Normalization is therefore not justified by a single need; it is a multiplier.
What it really speeds up, according to the requests below:

| Request | Effect of a normalized framework |
|---|---|
| move to GTK4, interface redesign | **strong**: widgets described by the modules are regenerated for GTK4, the web, or a simplified interface, instead of rewriting ~100 `gui_init` |
| beginner interface / workshops / AI | **strong**: same description, filtered and preset |
| multi-workstation catalog, server | **strong**: this is phase 0 |
| faster export, batches | moderate: declared compute needs -> scheduling; encoding still has to be parallelized separately |
| HDR, masks, stacking | low to moderate: mostly image processing work; a node graph would help stacking |
| new camera bodies | **none**: rawspeed / LibRaw / exiv2 |

Another effect cited by the user: **being able to rewrite some components in another language**
(Rust for the kernel, for example). A normalized interface becomes a stable boundary (C ABI)
behind which one component at a time is replaced, under the equivalence bench, without rewriting everything.
Known constraints: module parameters are stored as raw bytes (database, XMP, pipe
cache, see AGENTS.md), so their memory layout is part of the interface; introspection is
generated from the C structures (`tools/introspection/`); computation relies on OpenMP and OpenCL.

Proposed safeguard against the "second-system" effect: each normalization step must unlock
at least one visible and measurable capability (e.g. a generated interface for one module, a faster
export, two workstations on the same catalog), and stay under an equivalence bench.

## What darktable users ask for most and the roadmap does not cover

1. support for new camera bodies and formats (CR3 darktable-org/darktable#2170, HEIC darktable-org/darktable#4989, recent Sony);
2. interface redesign and move to GTK4 (darktable-org/darktable#15920, darktable-org/darktable#8497, darktable-org/darktable#19596);
3. HDR workflow and HDR export (darktable-org/darktable#18078, gain maps darktable-org/darktable#17399, UltraHDR);
4. generative fill in retouching (darktable-org/darktable#15006);
5. videos in the lighttable (darktable-org/darktable#11576);
6. packages and platforms (Homebrew macOS darktable-org/darktable#19982, AppImage, Wayland);
7. masks redesign (darktable-org/darktable#21905);
8. input devices (trackpad pinch, Wacom, MIDI);
9. culling ergonomics;
10. focus stacking (darktable-org/darktable#12582).

AI culling and fast selection are also a paid market at competitors (Aftershoot,
Photo Mechanic): a lead to keep in mind for a Helios front.

## Main evidence

### Shared catalog, multi-workstation
- darktable-org/darktable#5904 "Server/client mode" (2020): closed for inactivity; johnny-bit "Why would you
  want that at all?", aurelienpierre "too big of a change". Users kept asking for it.
- darktable-org/darktable#18599 "Sync library changes across multiple clients" (2025): `controversial`, closed; wpferguson:
  users already struggle to install darktable, a network database would be unbearable to
  maintain, "needs to be an add-on".
- darktable-org/darktable#18253 "working on two computers with one hard drive" (2025): 55 comments, open, no solution.
- darktable-org/darktable#21166 "cloud storage / VFS" (2026): `won't fix`.
- darktable-org/darktable#21340 "RFC: control of darktable over network" (2026): open; ralfbrown suggests extending DBus.
- Large libraries: 183,000 images, import of 128 photos in 1 min 53 versus 4 s on an empty database;
  389,000 images, 161 s at startup (discuss.pixls.us/t/22327). Fixed incrementally in SQLite
  (darktable-org/darktable#21849, darktable-org/darktable#21850).
- Lightroom Classic: single-user catalog, not on a NAS, request open for years.
- Capture One 16.8.2 (June 2026): multi-user sessions, LAN only, macOS first, beta.

### Travel laptop and home workstation

Several photographers keep a "travel darktable" on a laptop and reconcile it with the desktop later
(user's observation, 2026-10-10). Evidence: darktable-org/darktable#18253 "working on two computers with one hard drive"
(55 comments, open, no solution), darktable-org/darktable#18599 (closed as `controversial`), several pixls threads; the
manual page "using multiple computers" says sharing is not supported. What darktable offers today:
- XMP sidecars carried with the files, and "look for updated XMP files on startup"
  (`data/darktableconfig.xml.in`): last writer wins, no merge;
- local copies for offline work on images from an external drive (`common/image.c`,
  `dt_image_local_copy_set`, `dt_image_local_copy_synch`);
- nothing for presets and styles (`data.db`) or for absolute paths that differ between machines (darktable-org/darktable#17663).
Helios implication: the vault must be **offline-first** (disconnected replicas that sync later),
identify images by content hash rather than path, and record edits in a version graph where
concurrent edits become branches instead of conflicts.

### Export and batches
- darktable-org/darktable#17538 "Export Unusably Slow" (2024): 317 AVIF exports at 5% CPU; the encoders are
  single-threaded; ralfbrown: parallelizing exports is "very difficult", more memory.
- pixls: parallel export option removed years ago (/t/6279); running `darktable-cli` in parallel
  runs into the database lock, worked around with `--library :memory:` (/t/34302).
- The code confirms it: export is a sequential loop (`control/jobs/control_jobs.c`,
  `_control_export_job_run`).

### Learning curve and AI
- darktable-org/darktable#19084 "Make UI easier for beginners PLEASE" (2025): `not planned`; "If you want a lightroom
  clone then better stay with the original".
- RFC darktable-org/darktable#22024 "Who is darktable for" (2026, 17 comments): not for "someone wanting a 1 click
  solution" nor "someone who doesn't have time to invest in learning".
- AI: AI masks darktable-org/darktable#12295 (22 reactions, 65 comments) then ONNX subsystem darktable-org/darktable#20322 (112
  comments), shipped in 5.6, disabled by default; ethical debate (training data).
- Competition: Imagen ($0.05/photo), Aftershoot ($20 to $48/month), Evoto (credits, very poorly received).

### Collaboration
- No requests among darktable users.
- Lightroom: clients can neither filter by comments nor see the photographer's ratings
  (Adobe forum, threads merged without a fix). Capture One Live: comments tied to a session.
- Galleries and client approval: saturated and cheap market (Pixieset, ShootProof from $8 to $10/month);
  better integrated than rebuilt.

### Prior art: "git for photos"

Searched 2026-10-10 (web search; vendor claims not independently verified):
- **git + git-annex** by hand: raws in git-annex (hash pointers, `numcopies` redundancy, `whereis`),
  XMP sidecars committed to plain git after each darktable session. Used by a few pixls.us members
  (https://discuss.pixls.us/t/git-annex-use-in-workflow/2028, /t/31213); friction: command line only,
  sync surprises, no darktable integration, line-based merges of opaque XMP.
- **Design-file version control**, mostly gone: Pixelapse (YC, closed), LayerVault, Folio, Abstract
  and Kactus (git under the hood, Sketch only), PixelNovel Timeline (Photoshop, 2009). SnowFS
  (Snowtrack): binary-focused VCS for graphics, says git is unsuited to binaries; alpha.
- **Large-binary VCS** for ML data: Oxen.ai (git-like, Merkle trees, built from scratch), XetHub
  (Merkle trees + content-defined chunking, bought by Hugging Face in 2024), DVC, Git LFS.
- **Branching edit history**: Microsoft Research RevG (GIMP prototype: revision DAG with diff, branch,
  merge, conflict resolution); digiKam GSoC 2010 versioning design (not shipped as branches);
  Facefork (AI editor with a tree of edit nodes, vendor claim). Mainstream editors stop at
  snapshots and virtual copies.
Takeaway: the storage layer is solved (git-annex, Merkle/CDC object stores); what nobody ships is the
combination of a raw editor whose edits are diffable and mergeable, a version DAG per image, and a
photographer-friendly UX on top. Startups that tried it for designers mostly died on UX and
on being a layer outside the editor; Helios would have it inside the engine.

### Existing open source to know about
- **vkdt** (hanatos): Vulkan rewrite as a **node graph** (DAG), GPU only. Close to the AVS
  idea; to study before redesigning the pixelpipe.
  It **abandoned XMP** (src/db/readme.md): one `<image>.cfg` text file per image, which is the
  graph itself (`module:`, `connect:`, `param:filmcurv:01:contrast:0.634128`), ratings and labels in
  a small `vkdt.db` per folder, tags as folders of symbolic links; never writes to the
  originals. Limits it acknowledges: no global rating per image, no fast search on labels.
- **Ansel**: fork that simplifies darktable, without networking.
- **Immich, PhotoPrism**: self-hosted photo libraries without raw development: a natural outlet
  for a headless darktable engine.
- `mmcc-xx/darktable` branch `darktable-api` (cited in the roadmap): **not found** by web
  search; darktable-org/darktable#21340 mentions a `darktable-api` prototype on a fork [?]. To be rechecked.

## Measurements on a development laptop

Machine: AMD Ryzen 5 Pro 7535U laptop (6 cores, 12 threads, Radeon 680M iGPU), 30 GB; `darktable-cli` from commit 8c64bf7, **CPU
only** (`--disable-opencl`), `:memory:` library. Batch: 16 full-size renders (4 raws of 16 to 26
Mpx, settings from the `0035-multiple-modules` integration test), JPEG output ~5900x4100.
Script: a throwaway shell loop, not published.

| parallel processes | OpenMP threads each | batch duration |
|---|---|---|
| 1 | 12 | 185.6 s (11.6 s per render) |
| 2 | 6 | 182.7 s |
| 3 | 4 | 201.5 s |
| 4 | 3 | 237.4 s |
| 6 | 2 | 238.9 s |

Fixed cost of launching `darktable-cli`: ~1 s.

Reading:
- **on one machine, CPU rendering is already saturated** by internal parallelism (OpenMP):
  running several renders at the same time gains nothing, and loses up to 30% (memory, caches);
- the gain of a local parallel export would only come from the **single-threaded** stages (AVIF/JXL
  encoders, see darktable-org/darktable#17538: 5% CPU) and from input/output, not from rendering;
- beyond that, **only other resources** speed up a batch: graphics card, other machines.
  This is the argument for the distributed kernel, in numbers: ~12 s per photo here, i.e. ~20 min for 100 photos.
Not measured: OpenCL (disabled: an OpenCL run froze this machine), AVIF/JXL exports, time spent per module
(`-d perf`).

## XMP: an exchange format, not storage

The user's position (2026-10-10): XMP is a bad choice for storage, but a **good format
for distribution / export**. A darktable XMP holds two kinds of content:
- rating, labels, tags, title, author, GPS: standard namespaces (xmp, dc, lr...), read by all
  DAMs and photo software: **this is where XMP is good**;
- development history: `darktable:params` = raw bytes of the C structure in hexadecimal
  (or gzip + base64), unreadable outside darktable, frozen to the module's memory layout.
Helios target: storage in the vault (named settings, content-addressed versions, like vkdt's text
`.cfg`); XMP generated on export and distribution, read back on import, for interoperability
and darktable compatibility.

## Questions to settle

1. Target audience first: current darktable users (multi-workstation, speed, self-hosting)
   or studios (shared catalog, client approval)?
2. Should phase 0 aim for upstream by presenting it as a cleanup, or should we embrace a fork?
3. Validate the "intent -> workshop" AI with a mockup before any development?
4. Start again from the darktable engine or look at vkdt (node graph) for the Helios engine?
