# darktable pitfalls: GPU, performance and threading (for Helios)

Sources: darktable-org/darktable issues and PRs, swept 2026-10-12 by label
(OpenCL, AMD ROCm OpenCL, Intel OpenCL, Rusticl, scope: performance,
scope: threading, scope: hardware support) and keyword (tiling, headroom,
deadlock, memory leak, fast-relaxed-math, kernel compilation), sorted by
comment count. Threads were read with their comments unless marked "(title
only)". Helios state read from ROADMAP.md, docs/NEEDS.md,
crates/helios-core (engine, scheduler), crates/helios-gpu and
crates/helios-compute. Statements about wgpu defaults come from wgpu's
documentation, not from darktable.

---

### 1. Driver-reported GPU memory cannot be trusted
- What went wrong: darktable sizes tiling from `CL_DEVICE_GLOBAL_MEM_SIZE`.
  Drivers report it inconsistently: AMD APUs under ROCm/rusticl report the
  BIOS carve-out (512 MB) instead of the shared GART memory, so darktable
  tiles everything; others (Intel, rusticl on iGPUs) report nearly all host
  RAM (19 of 20 GB, or "host RAM minus 2 GiB"), so darktable does not tile
  and runs out. The rusticl maintainer confirmed this is a limitation of the
  OpenCL API: there is no reliable way to learn usable memory, and for
  unified memory darktable "simply doesn't know how much can be safely used".
- Evidence:
  https://github.com/darktable-org/darktable/issues/19513
  https://github.com/darktable-org/darktable/issues/19150
  https://github.com/darktable-org/darktable/issues/17210
  https://github.com/darktable-org/darktable/issues/20071
- Helios exposure: partly handled (2026-10-12). wgpu exposes no
  free-memory query at all; the only figures are adapter limits. Each
  device now has one memory budget that every buffer it holds is counted
  against: the per-device `vram_mb` setting, else
  `helios_gpu::default_budget` (four of the largest storage bindings, at
  most a quarter of host RAM, which covers unified memory). An allocation
  over it evicts device content, else runs the step on the CPU. Still
  open: the GUI's textures are not counted, and the budget does not
  shrink after a driver's out-of-memory error.
- Recommendation: never derive the budget from a driver number alone.
  Default to a conservative per-kind budget (integrated/unified: a fraction
  of host RAM; discrete: user-set or calibrated), expose it in device
  settings, and shrink it automatically after an out-of-memory error. Log
  the budget used at every render start (darktable's `-d pipe` "pipe
  starting" line is what made these reports diagnosable).

### 2. Probing free memory by allocating freezes some drivers
- What went wrong: the "tune OpenCL performance: memory size" option measured
  free device memory by allocating buffers until failure
  (`_opencl_get_unused_device_mem`). On NVIDIA 515/516 drivers this froze the
  darkroom ("working..." forever, kill -9 needed); on other machines the
  probe alone took 0.67 s per device. It was eventually reworked.
- Evidence:
  https://github.com/darktable-org/darktable/issues/12001
  https://github.com/darktable-org/darktable/issues/11606
- Helios exposure: not applicable today (no probing), but tempting once
  rule 1 bites.
- Recommendation: never probe by allocation. Use wgpu's out-of-memory error
  scope on real allocations, treat the error as a budget signal, and retry
  the step with smaller tiles or on the CPU.

### 3. Device limits smaller than an image force a less-tested path
- What went wrong: rusticl advertised lower per-allocation limits than
  Intel NEO, so demosaic switched to the tiled path; that path missed error
  checks and produced garbage instead of falling back ("any failure in tile
  should fallback"). The root cause turned out to be a rusticl compiler bug
  (unused kernel argument), but the limit difference was what exposed
  darktable's own bugs. darktable also lists "the largest requested buffer
  can be handled by driver/hardware" as a tiling criterion.
- Evidence:
  https://github.com/darktable-org/darktable/issues/16383
  https://github.com/darktable-org/darktable/pull/21694
- Helios exposure: handled (fixed 2026-10-12). `Gpu::open` used wgpu's
  default limits (256 MiB max buffer, 128 MiB max storage binding) while
  `fits()` checked jobs against `adapter.limits()`. The device is now
  opened with the adapter's limits (falling back to the adapter's buffer
  sizes, then wgpu's defaults, when a driver refuses, and saying so), the
  survey records those same limits, and `fits()` reads the opened device's
  limits once it is open. Kernels used to be dispatched along one
  dimension, 64 pixels a workgroup, so wgpu's 65535 workgroups per
  dimension capped a single dispatch near 4.2 Mpx whatever the buffer
  limits; they now run over a two-dimensional grid (fixed 2026-10-12,
  tested at 5.2 Mpx against the CPU).
- Recommendation: request `required_limits` from the adapter (at least
  `max_buffer_size` and `max_storage_buffer_binding_size`), and make
  `fits()` read the limits of the opened device, not of the adapter. Add a
  test that opens a device and asserts both agree.

### 4. The tiled path is a second-class code path
- What went wrong: many corruptions only happen when tiling kicks in, which
  depends on device memory, image size and settings, so core developers
  could not reproduce them: vertical stripes in the right half of exports
  (tiled scaling at the end of the pipe), garbled full-resolution output on
  512 MB laptop GPUs, a denoise margin that only some users saw ("I'm
  wondering if this is not a tiling issue?"). darktable eventually added a
  `--opencl-tiling` option to force tiling for debugging.
- Evidence:
  https://github.com/darktable-org/darktable/issues/16984
  https://github.com/darktable-org/darktable/issues/21617
  https://github.com/darktable-org/darktable/issues/3249
  https://github.com/darktable-org/darktable/issues/22241 (title only)
  https://github.com/darktable-org/darktable/pull/21694
- Helios exposure: at risk later. The engine is region based (modules
  declare an overlap) which is a sound base, but GPU steps currently run
  whole regions in one buffer; no GPU tiling exists yet.
- Recommendation: make "tiled equals untiled" a property test for every
  module, CPU and GPU, with a forced tiny tile size (odd sizes, tiles
  smaller than the overlap). Run it in CI on lavapipe so it never depends on
  the developer's VRAM.

### 5. Tiling breaks modules that need whole-image statistics
- What went wrong: in retouch, OpenCL fast tiling calls `process_cl()` once
  per strip; a one-shot token lets the first strip publish its min/mean/max
  as the frame's auto levels, and the GUI writes them into the history.
  Demosaic's dual mode, details mask and green equilibration likewise had to
  run untiled, forcing CPU fallbacks on small GPUs until demosaic got its
  own internal tiling.
- Evidence:
  https://github.com/darktable-org/darktable/issues/22100
  https://github.com/darktable-org/darktable/pull/19217
- Helios exposure: at risk. The module contract declares an overlap, but no
  "global" input need yet.
- Recommendation: let a module declare a global dependency (statistics,
  histogram, guide image) as a separate node computed on the whole image
  (or a proxy), whose result is an input of the per-tile node. Never let a
  per-tile call publish anything.

### 6. A GPU is not always faster: overlap and transfers can lose to the CPU
- What went wrong: diffuse-or-sharpen and laplacian highlights have large
  overlaps; on small GPUs they tile so much that the CPU would have been
  faster. Users asked for per-module "disable OpenCL" switches; darktable
  refused a static per-module setting ("runtime defined parameters are not
  taken into account") and added a per-device "advantage" factor plus a
  memory-weighted estimate deciding CPU versus GPU per step.
- Evidence:
  https://github.com/darktable-org/darktable/pull/12545
  https://github.com/darktable-org/darktable/issues/5669
  https://github.com/darktable-org/darktable/issues/11606
- Helios exposure: partly handled. `helios-compute` calibrates devices on
  reference modules over a fixed 512 px tile, but the engine decides GPU use
  per module (`acceleratable()`), not per step size and overlap.
- Recommendation: decide per step with a cost model: calibrated throughput
  times processed bytes including overlap and tile count, plus transfer
  cost for any host/device crossing. Keep calibration per module family and
  record why a step went where it did.

### 7. Host/device transfers dominate, and their cost is driver specific
- What went wrong: stitching tiles in host memory was "quite costly
  especially for OpenCL"; darktable moved to full-width strips and
  on-device copies, and removed pinned tiling because no reliable gain could
  be shown across drivers. On NVIDIA GB10, readback into a cold `malloc`
  buffer was extremely slow until destination pages were pre-faulted.
- Evidence:
  https://github.com/darktable-org/darktable/pull/19217
  https://github.com/darktable-org/darktable/pull/21694
  https://github.com/darktable-org/darktable/pull/21069
  https://github.com/darktable-org/darktable/issues/17983
  https://github.com/darktable-org/darktable/pull/21095
- Helios exposure: partly handled. Images stay on the device between
  accelerated steps and the viewport shares the device (good), but readback
  creates a new MAP_READ buffer per call and every CPU step in the middle of
  a GPU chain costs a round trip.
- Recommendation: pool staging buffers per size class, keep tiles on the
  device and stitch there, count bytes crossed per render in the engine
  stats, and bias the scheduler against CPU islands inside GPU chains.

### 8. Vendor compilers produce wrong pixels, not errors
- What went wrong: the same kernel gave correct output on one driver and
  wrong output on another: rusticl mishandled an unused kernel argument
  (darktable removed the argument), ROCm amplified local laplacian (worked
  around with a vendor `#ifdef`), Intel on macOS broke local contrast in one
  release, Intel Linux rendered images almost white, and Intel Arc needed an
  atomics workaround in the laplacian kernel.
- Evidence:
  https://github.com/darktable-org/darktable/issues/16383
  https://github.com/darktable-org/darktable/issues/3756
  https://github.com/darktable-org/darktable/issues/7544
  https://github.com/darktable-org/darktable/issues/11201
  https://github.com/darktable-org/darktable/issues/20257
- Helios exposure: partly handled. `helios-gpu::check` compares a GPU
  variant with the CPU reference within a tolerance, and content keys
  include the accelerator id, but the check runs in tests, not on the
  user's device. WGSL goes through naga, then the vendor's SPIR-V or MSL
  compiler, so vendor bugs still apply (guess: fewer than with OpenCL C,
  not none).
- Recommendation: run the CPU-versus-GPU check for each GPU module on the
  user's device at calibration (small fixed tile), store a per-device,
  per-module denylist with the driver version, and route denied modules to
  the CPU. Keep kernels free of unused bindings and avoid float atomics.

### 9. One physical GPU seen twice through two drivers
- What went wrong: users with two OpenCL platforms installed for one card
  (ROCm plus legacy AMD-APP, Mesa plus vendor) got two darktable devices for
  one GPU, leading to stripes, crashes and memory confusion; the maintainer:
  "Two drivers on one device certainly is a problem".
- Evidence:
  https://github.com/darktable-org/darktable/issues/16984
  https://github.com/darktable-org/darktable/issues/20957
  https://github.com/darktable-org/darktable/issues/19150
- Helios exposure: at risk. `WgpuAdapters` lists every adapter wgpu sees,
  and Vulkan can expose the same card through two ICDs (for example RADV
  and AMDVLK), and also through GL; ids are backend plus PCI ids, so the
  two would look like distinct devices.
- Recommendation: group adapters by PCI vendor/device id (and LUID where
  available), keep one per physical GPU by a fixed backend and driver
  preference, list the others as alternatives in diagnostics, and never
  schedule two at once on the same card.

### 10. Quirk lists keyed on names break when vendors rename
- What went wrong: Intel compute-runtime 26.35 renamed its platform; the
  name-derived key fell back to "other", disabled by default, and darktable
  then persisted `opencl=FALSE`, so later starts ran silently on the CPU.
  darktable also passed NVIDIA-specific compile flags based on the vendor
  id to the nouveau/rusticl stack, where they did not apply.
- Evidence:
  https://github.com/darktable-org/darktable/issues/22386
  https://github.com/darktable-org/darktable/issues/16564
  https://github.com/darktable-org/darktable/issues/20957
- Helios exposure: not yet (no quirk list), at risk as soon as one is
  added.
- Recommendation: key quirks on (vendor id, device id range, driver id,
  driver version range) from `wgpu::AdapterInfo`, never on display names;
  an unknown device is enabled with the conservative defaults, never
  silently disabled, and a disabled state is never persisted without
  telling the user why.

### 11. Driver crashes during initialisation take the whole app down
- What went wrong: darktable crashed at start inside `amdocl64.dll`, inside
  the ROCm platform check, or failed to start with Intel drivers; users
  could not even reach the preferences to turn OpenCL off. darktable added
  blacklisting of the legacy AMD-APP driver and stability commits around
  platform checks.
- Evidence:
  https://github.com/darktable-org/darktable/issues/20957
  https://github.com/darktable-org/darktable/issues/14061
  https://github.com/darktable-org/darktable/issues/20037 (title only)
- Helios exposure: at risk. The GUI and the engine share one wgpu device;
  a driver crash during adapter enumeration kills the GUI too.
- Recommendation: write a "GPU init in progress" marker before enumerating
  and opening devices and clear it after; if the marker survives a crash,
  start on the CPU (or software adapter for the GUI) and say so. Consider
  enumerating adapters in a short-lived child process at first start and
  after driver updates.

### 12. Hangs, GPU resets and system freezes
- What went wrong: OpenCL exports froze whole systems (NVIDIA open
  modules), Intel Arc triggered Xe engine resets on one kernel, an overexposure
  indicator hung Arc B580, and `clWaitForEvents` returned
  CL_OUT_OF_RESOURCES on an RTX 5070 Ti until the wait logic changed.
  darktable's answer is mostly "driver issue", but the app cannot recover.
- Evidence:
  https://github.com/darktable-org/darktable/issues/20174
  https://github.com/darktable-org/darktable/issues/20257
  https://github.com/darktable-org/darktable/issues/19032
  https://github.com/darktable-org/darktable/issues/22529
- Helios exposure: partly handled (2026-10-12). Every wait has a deadline
  (`helios_gpu::DEFAULT_TIMEOUT`, 20 s), a device-lost callback is
  registered, and a hung or lost device is rebuilt once and the render
  replayed, else the engine renders on the CPU for the rest of the session
  and says so. Still open: dispatches are not split under the OS
  watchdog, and the GUI's window keeps the lost device (the viewport
  blanks until a restart).
- Recommendation: split long dispatches so each submission stays well under
  the OS watchdog (Windows TDR is about 2 s by default; guess for a budget:
  100 ms), poll with a timeout, register `set_device_lost_callback`, and on
  loss recreate the device, re-upload from the cache, and retry the step
  on the CPU.

### 13. No per-step CPU fallback, or a silent one
- What went wrong: darktable falls back to CPU code when OpenCL fails
  (memory, kernel error), which keeps renders alive but hides problems:
  users saw stutter or slow exports caused by "could not allocate oversize
  buffer" fallbacks, and guided filter silently fell back for lack of
  memory. The fallback also needs the error paths to restore state, which
  retouch did not.
- Evidence:
  https://github.com/darktable-org/darktable/issues/20055
  https://github.com/darktable-org/darktable/issues/11606
  https://github.com/darktable-org/darktable/issues/20071
  https://github.com/darktable-org/darktable/issues/22081
- Helios exposure: handled (2026-10-12). A step whose kernel fails (does
  not compile, does not run, out of memory once eviction freed nothing) is
  computed on the CPU; the engine publishes `engine.gpu_fell_back`
  (node, module, failure kind, reason) and counts it
  (`Counters::fallbacks`). Not yet surfaced in the GUI.
- Recommendation: on a GPU error for a step, retry that step on the CPU,
  publish a bus event (device, module, reason, cost), count it in engine
  stats, and surface repeated fallbacks to the user. Cache keys already
  separate CPU and GPU content, so the retry is cache-safe.

### 14. Sharing the GPU with the desktop and other apps
- What went wrong: darktable keeps a "headroom" for other users of the GPU
  (400-600 MB); with a browser or compositor active this was too little and
  a 100% zoom in culling led to a blue screen. After closing darktable on
  ROCm, games would not launch until darktable closed its OpenCL library;
  on ROCm iGPUs darktable did not even terminate.
- Evidence:
  https://github.com/darktable-org/darktable/issues/17684
  https://github.com/darktable-org/darktable/issues/20048
  https://github.com/darktable-org/darktable/pull/20059
  https://github.com/darktable-org/darktable/issues/15931
- Helios exposure: at risk, and sharper than in darktable: the GUI's own
  textures (egui, viewport tiles, thumbnails) live on the same device as
  the engine's buffers.
- Recommendation: one memory budget per device that counts GUI textures,
  engine cache and in-flight jobs together; explicit teardown order at exit
  (stop jobs, drop buffers, drop device); a test that the process exits
  within a deadline with the GPU enabled.

### 15. Concurrent pipelines each assume the whole budget
- What went wrong: culling at 100% started two pipelines back to back, and
  the "should we tile" logic "allows every pipe the whole lot", so memory
  use doubled and an 8 GB machine crashed. Parallel thumbnail generation
  ran into the same question of how many pipes can share memory.
- Evidence:
  https://github.com/darktable-org/darktable/issues/17684
  https://github.com/darktable-org/darktable/issues/5673
- Helios exposure: partly handled. The scheduler acquires named resources
  (GPU memory among them) atomically before a job starts, which is the
  right model; the engine cache has its own budget. Not verified that every
  render path declares its GPU memory need.
- Recommendation: make every render declare a memory estimate (from the
  plan: buffers, overlap, tile count) and acquire it from the scheduler;
  tiles shrink until the estimate fits what is free, never the whole
  device.

### 16. Huge images: input and output buffers are the floor
- What went wrong: a 700 MP scan needs at least 2 x 4 channels x 4 bytes
  per pixel (about 22 GB) for one module, because tiling only reduces a
  module's extra buffers, not the full-size input and output; darktable
  then swaps or the OOM killer strikes before it can abort gracefully.
- Evidence:
  https://github.com/darktable-org/darktable/issues/18589
  https://github.com/darktable-org/darktable/issues/17684
- Helios exposure: partly handled. Rendering is region based, so a full
  frame is not required by design, but a full-resolution export or a
  global-statistics node will still materialise whole images.
- Recommendation: estimate peak memory from the plan before starting, fail
  early with a clear message, and stream exports by region (strips) with
  only global nodes computed on a proxy or in a separate pass.

### 17. Per-thread buffers sized for a thread count someone else changed
- What went wrong: GraphicsMagick's initialisation reset OpenMP's thread
  count after darktable set it, so parallel loops ran with more threads
  than the per-thread scratch pools held, corrupting the heap in
  darktable-cli. A related darktable-cli report of incomplete exports was
  traced to the number of OpenMP threads used.
- Evidence:
  https://github.com/darktable-org/darktable/issues/22436
  https://github.com/darktable-org/darktable/issues/18127
- Helios exposure: low risk. Rust with a scoped thread pool and
  per-iteration state (`map_init`, thread-locals) avoids this class, as
  long as no module indexes a scratch array by a thread index.
- Recommendation: forbid scratch arrays indexed by thread id in modules
  (review rule or lint); use per-task allocation or `map_init`. Run each
  module's tests with 1, 2 and many threads.

### 18. GUI and pipeline sharing mutable state: races and lock inversions
- What went wrong: modules keep `gui_data` read by pipe threads and the GTK
  thread; an audit filed many defects: a pointer copied under the lock and
  dereferenced after release while another pipe frees it, flags read
  outside the lock, an AB-BA deadlock between `gui_lock` and
  `history_mutex` freezing the darkroom, a failure path stranding a
  GUI handshake. Import also blocked the UI through `dt_lock_image`.
- Evidence:
  https://github.com/darktable-org/darktable/issues/22060
  https://github.com/darktable-org/darktable/issues/22133
  https://github.com/darktable-org/darktable/issues/22066
  https://github.com/darktable-org/darktable/issues/22081
  https://github.com/darktable-org/darktable/issues/7111
- Helios exposure: handled by design for modules (pure, no GUI state,
  outputs addressed by content) and for scheduler resources (acquired
  together, no partial holds). At risk in the GUI/engine glue (readouts,
  pickers, histograms) if it reintroduces shared mutable state.
- Recommendation: anything a module measures for the GUI (levels,
  histograms, picker values) is an output port of the graph, delivered by
  the engine as content; the GUI never shares locks with render threads.
  Write the lock order down where locks are unavoidable.

### 19. One pipe waiting on another pipe's side data
- What went wrong: the full pipe syncs on data produced by the preview pipe
  (`dt_dev_sync_pixelpipe_hash`, bilateral grids, preview data), which
  creates waits, "inconsistent output" messages, and use-after-free when the
  preview pipe replaces the data mid-read.
- Evidence:
  https://github.com/darktable-org/darktable/issues/22060
  https://github.com/darktable-org/darktable/issues/22066
- Helios exposure: handled if the proxy-then-exact design keeps proxy
  results as ordinary cached content with keys.
- Recommendation: a value the exact render needs from the proxy render is
  an explicit graph dependency with a content key, never a side channel.

### 20. UI work triggered by processing: redundant redraws and reruns
- What went wrong: the scopes recompute on every preview run (fast and
  standard runs give different data, so the histogram flickers); any widget
  update repaints the whole window; a thumbnail helper loaded the image and
  ran a full pipe just to compute a final size; resizable panels checking
  their size on every paint cost GUI performance.
- Evidence:
  https://github.com/darktable-org/darktable/issues/16929
  https://github.com/darktable-org/darktable/issues/17243
  https://github.com/darktable-org/darktable/issues/6522
- Helios exposure: at risk in the GUI (egui repaints every frame it is
  asked to); the engine's content cache already removes recomputation of
  unchanged steps.
- Recommendation: scopes and readouts subscribe to engine results by key
  and redraw only when the key changes; sizes and metadata come from the
  plan, never from a render; request repaints only on new content.

### 21. Kernel compilation at startup and stale kernel caches
- What went wrong: darktable compiled all OpenCL kernels at start even with
  OpenCL disabled, taking 55 s on Windows where its symlink-based kernel
  cache did not work; "delete the cached kernels" became a standard
  troubleshooting step for AMD start-up problems.
- Evidence:
  https://github.com/darktable-org/darktable/issues/3384
  https://github.com/darktable-org/darktable/issues/14061
  https://github.com/darktable-org/darktable/issues/15931
- Helios exposure: partly handled. Pipelines are compiled lazily and kept
  in memory per `Gpu`; the calibration bench warms them. No on-disk
  pipeline cache (`cache: None`), so every start recompiles on first use.
- Recommendation: if an on-disk cache is added (wgpu `PipelineCache` on
  Vulkan), key it on adapter, driver version and kernel source hash, store
  it with plain files (no symlinks), and offer a "clear GPU caches" command.
  Compile in the background, never on the UI thread.

### 22. Performance regressions and benchmarks that move under you
- What went wrong: a 12% darktable-bench regression turned out partly to be
  a new "OpenCL fast mode" option that was off by default, so old and new
  results were not comparable; VNG demosaic regressed on OpenCL in 5.4 and
  caused visible stutter; maintainers rely on users' hand-run exports to
  spot regressions.
- Evidence:
  https://github.com/darktable-org/darktable/issues/20865
  https://github.com/darktable-org/darktable/issues/20055
  https://github.com/darktable-org/darktable/issues/11606
- Helios exposure: partly handled (nextest timings in CI, a calibration
  bench); no engine throughput bench with recorded settings.
- Recommendation: a fixed engine benchmark (graph, image, scale, device,
  settings hash) recorded per commit on lavapipe and CPU, with the settings
  printed in the result so a changed default is visible.

### 23. Developers cannot reproduce on the hardware users have
- What went wrong: "no darktable dev is on AMD"; AMD and Intel reports
  stayed open for months, were closed as driver issues, or waited for a
  developer to buy hardware. Root causes were only found when a driver
  developer joined (rusticl) and when debug tools existed (`--dump-pipe`,
  `--dump-diff-pipe`, `-d tiling`).
- Evidence:
  https://github.com/darktable-org/darktable/issues/18699
  https://github.com/darktable-org/darktable/issues/17210
  https://github.com/darktable-org/darktable/issues/16383
  https://github.com/darktable-org/darktable/issues/18127
- Helios exposure: at risk (development happens on one AMD laptop).
- Recommendation: ship a self-test command that runs the CPU-versus-GPU
  check and the bench on the user's device and produces a shareable report
  (adapter info, driver, limits, per-module max error, timings); keep a
  per-step dump of inputs and outputs for any node, and a diff against the
  CPU reference.

### 24. Platform GPU APIs get deprecated under you
- What went wrong: Apple deprecated OpenCL; Big Sur broke darktable's
  OpenCL library loading until a path workaround was found, and macOS
  OpenCL quality varied per GPU and OS release.
- Evidence:
  https://github.com/darktable-org/darktable/issues/6918
  https://github.com/darktable-org/darktable/issues/7544
- Helios exposure: handled. wgpu targets Metal on macOS and Vulkan
  elsewhere, both actively maintained.
- Recommendation: keep the CPU reference first-class so a backend loss is a
  slowdown, never a missing feature; avoid backend-specific extensions in
  module kernels.

---

## Top 5 for Helios now

1. **Fix the device limits mismatch (pitfall 3).** Request the adapter's
   buffer and binding limits in `Gpu::open`, and make `fits()` use the
   opened device's limits; today a 24 MP f32 RGBA image already exceeds
   wgpu's default 128 MiB binding.
2. **Per-step CPU fallback with events (pitfall 13).** Done 2026-10-12.
3. **Hang and device-loss handling (pitfall 12).** Done 2026-10-12 for the
   engine; the GUI's window does not follow a rebuilt device yet.
4. **One memory budget per device including GUI textures, declared by every
   job (pitfalls 1, 14, 15).** The engine's buffers are counted against
   one budget per device (2026-10-12); GUI textures and per-job
   declarations are still open, as is shrinking on out-of-memory errors.
5. **Design tiling now, with tests (pitfalls 4, 5).** A "tiled equals
   untiled" property test per module with forced tiny tiles, and a declared
   "global" need computed as its own node, before GPU tiling code exists.
