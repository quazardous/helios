# Wide-gamut preview on Wayland: declaring the surface's colour encoding

Research note, 2026-10-12. Helios context: GUI on egui 0.36 / eframe 0.36.2 /
egui-wgpu 0.36.2, wgpu 30.0.1, winit 0.30.13, Vulkan on Linux. Today the
viewport (`crates/helios-gui/src/viewport.rs`) converts linear Rec.2020 to
sRGB primaries (`display_matrix()`), clamps per channel and applies the
piecewise sRGB encoding in the shader, so on Wayland it shows correct
colours, limited to the sRGB gamut. The goal: show the wider gamut by
telling the compositor how the pixels are encoded, and letting it map them
to the monitor. The compositor does the monitor conversion, never Helios
(see `docs/darktable-pitfalls/colour.md`, pitfalls 1, 2, 12).

What is wired since, and what works where: [wide-gamut.md](wide-gamut.md).

Sources were read on the date above. "Unverified" marks what was inferred
and not checked in code or on a machine. Compositor and Mesa details come
from their main branches on that date unless a release is named.

---

## TL;DR

- wgpu 30 (July 2026) added `SurfaceConfiguration::color_space`
  (gfx-rs/wgpu#9658). On Linux, Mesa's Vulkan WSI (Mesa 25.1 and later)
  turns the chosen `VkColorSpaceKHR` into a `wp_color_management_v1` image
  description on the window's `wl_surface`. With both, Helios can declare a
  wide-gamut encoding without writing any Wayland code.
- What stands in the way is egui-wgpu, not wgpu. eframe always configures
  `Rgba8Unorm`/`Bgra8Unorm` with `SurfaceColorSpace::Auto` (resolved to
  sRGB). egui's shader also chooses its output encoding from
  `format.is_srgb()`, so on a linear float target it would write
  gamma-encoded UI colours. Both need a small change in egui-wgpu/eframe.
- **Recommendation: path (a), the whole window in a wgpu colour space.**
  Start with scRGB (`Rgba16Float` + `ExtendedSrgbLinear`): the viewport
  writes linear BT.709 values with negatives kept and no clamp, and egui uses
  its linear output entry point. This works through Mesa on KWin, Hyprland
  and Weston. GNOME offers `DisplayP3` instead. The same code also gives
  scRGB on Windows/DX12 and EDR on macOS.
- Path (b), our own subsurface tagged through the protocol (what darktable
  does), stays the fallback design. If the subsurface is drawn through a
  second wgpu/Vulkan swapchain, Mesa may already have created the surface's
  only colour-management object, and tagging it again is a fatal protocol
  error. Doing it safely means presenting our own buffers (SHM like
  darktable, or dmabuf), which is much more work.
- Upstream: egui-wgpu (surface format and colour-space selection; output
  transfer separate from `is_srgb`), wgpu (a `Bt2020Linear` colour space,
  since Mesa already offers `VK_COLOR_SPACE_BT2020_LINEAR_EXT`), winit#4131
  (colour-management getters). Locally: a `DisplayTarget` and a surface
  feedback probe for diagnostics.

---

## 1. The protocol: `wp_color_management_v1`

### Status

- Merged into wayland-protocols **1.41** (February 2025) under `staging/`.
  It is still staging on wayland-protocols main (latest tag 1.49,
  2026-06-07). `stable/` holds only linux-dmabuf, presentation-time,
  tablet, viewporter and xdg-shell. Interface `wp_color_manager_v1` is at
  **version 3**. Version 2 deprecated the `srgb` and `ext_srgb` transfer
  functions and added `compound_power_2_4`, `absolute_no_adaptation`,
  `ready2` and `preferred_changed2`. Version 3 added `create_windows_bt2100`.
  - https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/color-management/color-management-v1.xml
  - https://wayland.app/protocols/color-management-v1
  - https://www.phoronix.com/news/Wayland-Protocols-1.41
  - https://www.collabora.com/news-and-blog/news-and-events/12-years-of-incubating-wayland-color-management.html
- The protocol text still says "testing phase": changes stay backward
  compatible through version bumps.

### What a client does

1. Bind `wp_color_manager_v1` from the registry. Collect the
   `supported_intent`, `supported_feature`, `supported_tf_named` and
   `supported_primaries_named` events until `done`. Use only what was
   advertised: an unadvertised value is a protocol error, and a protocol
   error kills the connection.
2. `create_parametric_creator` (needs feature `parametric`). Then:
   - `set_primaries_named` (for example `bt2020`, `display_p3`, `srgb`) or
     `set_primaries` with explicit CIE xy (feature `set_primaries`);
   - `set_tf_named` (for example `gamma22`, `ext_linear`, `st2084_pq`, `hlg`)
     or `set_tf_power` (feature `set_tf_power`);
   - optionally `set_luminances(min*10000, max, reference)` (feature
     `set_luminances`), `set_mastering_display_primaries` /
     `set_mastering_luminance` (HDR metadata, with feature
     `extended_target_volume` when the target exceeds the primaries), and
     `set_max_cll` / `set_max_fall`;
   - `create` returns a `wp_image_description_v1`. Wait for `ready`/`ready2`
     (or `failed`). Setting a description that is not ready is a protocol
     error.
3. `get_surface(wl_surface)` returns a `wp_color_management_surface_v1`.
   **Only one may exist per `wl_surface`**: a second one raises
   `surface_exists`. Then
   `set_image_description(description, render_intent)`. This state is
   double-buffered and applies on `wl_surface.commit`. The intent must be
   advertised; only `perceptual` is mandatory.
4. Optionally, `get_surface_feedback(wl_surface)`. It is not exclusive, so
   it can live next to someone else's colour surface. `get_preferred` /
   `get_preferred_parametric` returns the compositor's preferred description
   for that surface, and `preferred_changed2` signals changes, for example
   when the window moves to another output. `get_information` on a
   description returns primaries, TF, luminances and target volume. KWin's
   developer advises using this instead of tracking `wl_output`s
   (https://github.com/darktable-org/darktable/issues/19977).

Semantics that matter for Helios:

- An untagged surface is "compositor-defined, should be handled as sRGB".
- Named `gamma22` is the sRGB *reference display* EOTF (IEC 61966-2-1).
  The piecewise sRGB curve is `compound_power_2_4`, offered only by some
  compositors. Helios currently encodes with the piecewise curve, while
  KWin/wlroots treat untagged content as gamma 2.2. The two differ visibly
  only in deep shadows, but a declared surface should use the curve it
  declares.
- `ext_linear` with sRGB primaries is scRGB: 1.0 is reference white, and
  values below 0 or above 1 are allowed, which needs feature
  `extended_target_volume` to mean anything outside the gamut.
- `st2084_pq` implies reference white = **203 cd/m²** (BT.2408); `hlg` too,
  at a nominal 1000 cd/m² peak. Compositors "anchor" content: the reference
  white of every surface comes out at the same level.

### Compositor support (from source, read 2026-10-12)

| Compositor | First release with `wp_color_manager_v1` | Relative intent | `extended_target_volume` | Named TFs advertised | Notes |
|---|---|---|---|---|---|
| KWin | 6.3.2 (6.3.0 shipped `xx_color_management_v4`) | yes (+ abs, rel_bpc) | yes | gamma22, pq, ext_linear, bt1886, hlg, cp2.4 (v2+) | never advertises `srgb` TF; manager v3; also `windows_scrgb` |
| Mutter / GNOME | 48.0 | **no** (perceptual only) | **no** | gamma22, gamma28, pq, bt1886, ext_linear; `srgb` to v1 clients, `compound_power_2_4` to v2+ | primaries srgb, bt2020, ntsc, pal, display_p3; also ICC |
| wlroots / sway | wlroots 0.19.0 (protocol); full rendering in 0.20 (secondary source, unverified) | no (sway sets perceptual only) | no (sway) | taken from the renderer: gamma22, pq, ext_linear, bt1886, cp2.4; `srgb` is asserted away | primaries srgb, bt2020 only; manager v2 |
| Hyprland | 0.48.0 (0.47 had `xx_v4`) | only with CM debug enabled | yes | wide list; `srgb`/`ext_srgb` to v1 clients | primaries incl. display_p3, adobe_rgb |
| Weston | 15.0.0 (14.0 had `xx_v4`); needs `[core] color-management=true` and the GL renderer | yes (+ abs, rel_bpc) | yes | gamma22, gamma28, pq, ext_linear | color-lcms backend; ICC too |

Code read:
- KWin: https://invent.kde.org/plasma/kwin/-/blob/master/src/wayland/colormanagement_v1.cpp (bind handler, lines ~23-60)
- Mutter: https://gitlab.gnome.org/GNOME/mutter/-/blob/main/src/wayland/meta-wayland-color-management.c (`color_manager_send_supported_events`, ~1833-1880)
- wlroots: https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/master/types/wlr_color_management_v1.c ; sway: https://github.com/swaywm/sway/blob/master/sway/server.c (~630-658)
- Hyprland: https://github.com/hyprwm/Hyprland/blob/main/src/protocols/ColorManagement.cpp (~15-70)
- Weston: https://gitlab.freedesktop.org/wayland/weston/-/blob/main/libweston/color-lcms/color-lcms.c (~508-536)

The column that matters most for an editor is relative intent. A design
that requires the relative colorimetric intent (as darktable's PR does)
switches itself off on GNOME, sway and default Hyprland.

---

## 2. Path (a): the swapchain colour space

### wgpu 30

- `wgpu::SurfaceColorSpace` and `SurfaceConfiguration::color_space` came in
  with **gfx-rs/wgpu#9658** (merged 2026-06-26, released in wgpu 30.0.0 on
  2026-07-01). `Surface::display_hdr_info()` (#9739) followed. Capabilities
  list, per format, a `SurfaceColorSpaces` bitset (`format_capabilities`).
  The variants are `Auto` (default), `Srgb`, `ExtendedSrgbLinear` (scRGB),
  `DisplayP3`, `Bt2100Pq`, `Bt2100Hlg`, `ExtendedSrgb` and
  `ExtendedDisplayP3` (Metal/web only).
  - `wgpu-types-30.0.1/src/surface.rs` (enum ~line 242, docs 140-415)
  - https://github.com/gfx-rs/wgpu/pull/9658 ; https://github.com/gfx-rs/wgpu/pull/9739
  - Example: https://github.com/gfx-rs/wgpu/tree/trunk/examples/standalone/03_hdr_surface
- On Vulkan, `wgpu-hal-30.0.1/src/vulkan/conv.rs:183-210` maps
  `SRGB_NONLINEAR`, `EXTENDED_SRGB_LINEAR`, `EXTENDED_SRGB_NONLINEAR`,
  `DISPLAY_P3_NONLINEAR`, `HDR10_ST2084` and `HDR10_HLG`. It **drops**
  `BT2020_LINEAR_EXT`, `ADOBERGB_*` and `PASS_THROUGH_EXT` (test
  `unknown_color_spaces_are_dropped`, ~1110). wgpu enables
  `VK_EXT_swapchain_colorspace` (`vulkan/instance.rs:280`).
- `display_hdr_info()` returns nothing for Vulkan outside Windows, so on
  Wayland the display's luminance or primaries must come from the protocol
  (surface feedback, see below).
- `as_hal`: none of this needs it any more. Raw access would only help to
  reach a colour space wgpu drops (BT2020_LINEAR, PASS_THROUGH). That would
  mean building our own `VkSwapchainKHR` with ash and wrapping its images
  (`texture_from_raw`), with our own acquire/present/sync. That is not clean.
  The clean route is a small wgpu PR adding `Bt2020Linear` (and maybe
  `PassThrough`).

### Mesa WSI

- Mesa **25.1** first maps `VK_EXT_swapchain_colorspace` (and
  `VK_EXT_hdr_metadata`) onto `wp_color_management_v1` (author Xaver Hugl,
  KWin). Confirmed: tag `mesa-25.1.0` contains the code, and main is at
  26.2.x. https://www.phoronix.com/news/Mesa-Vulkan-WSI-HDR-CM ,
  https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/src/vulkan/wsi/wsi_common_wayland.c
- The mapping (`colorspace_mapping[]`, ~line 1037):

  | VkColorSpaceKHR | primaries | TF | extra condition |
  |---|---|---|---|
  | SRGB_NONLINEAR | srgb | **srgb** (deprecated) | always listed |
  | DISPLAY_P3_NONLINEAR | display_p3 | **srgb** | |
  | EXTENDED_SRGB_LINEAR | srgb | ext_linear | needs `extended_target_volume` |
  | DISPLAY_P3_LINEAR | display_p3 | ext_linear | |
  | BT709_LINEAR / BT709_NONLINEAR | srgb | ext_linear / bt1886 | |
  | BT2020_LINEAR | bt2020 | ext_linear | |
  | HDR10_ST2084 | bt2020 | st2084_pq | HDR metadata used |
  | HDR10_HLG | bt2020 | hlg | HDR metadata used |
  | ADOBERGB_LINEAR | adobe_rgb | ext_linear | |
  | PASS_THROUGH | none | none | Mesa does not tag the surface |

  A colour space is listed only if the compositor advertised its primaries
  and TF. Mesa binds the manager at **version 1** and always uses the
  **perceptual** intent.
- Two consequences:
  - Mesa still asks for the `srgb` TF, which KWin, wlroots and Weston do not
    advertise. So `DisplayP3` is **not offered on KWin, sway or Weston**,
    and `SRGB_NONLINEAR` stays untagged there. On Mutter and Hyprland, which
    send `srgb` to v1 clients, it is offered and tagged.
  - `needs_color_surface()` (~1255) and swapchain creation (~4057): Mesa
    calls `wp_color_manager_v1_get_surface` on the window's `wl_surface` for
    every non-pass-through colour space, **including plain sRGB** when the
    compositor advertises srgb+srgb. After that, nobody else may create a
    colour surface on that `wl_surface`. That rules out "Mesa swapchain +
    our own tagging" on the same surface.
- What Helios gets through wgpu 30 + Mesa on each compositor (derived from
  the two tables, **unverified on hardware**):

  | Compositor | Srgb | ExtendedSrgbLinear (scRGB) | DisplayP3 | Bt2100Pq | Bt2100Hlg |
  |---|---|---|---|---|---|
  | KWin | untagged | yes | no | yes | yes |
  | Mutter | tagged | no | yes | yes | no |
  | sway | untagged | no | no | yes | no |
  | Hyprland | tagged | yes | yes | yes | yes |
  | Weston (CM on) | untagged | yes | no | yes | no |

  If wgpu exposed `BT2020_LINEAR` (Mesa lists it wherever `bt2020` and
  `ext_linear` are advertised), it would add a wide SDR option on *every*
  compositor above, sway and GNOME included.
- NVIDIA's proprietary driver did not implement `VK_EXT_swapchain_colorspace`
  on Wayland as of a 2025 forum report
  (https://forums.developer.nvidia.com/t/vulkan-extensions-needed-for-hdr-is-missing/334268).
  Its current state is **unverified**, and Helios must fall back to `Srgb`.

### egui-wgpu / eframe

- `egui-wgpu-0.36.2/src/lib.rs:422-438` (`preferred_framebuffer_format`)
  picks `Rgba8Unorm`/`Bgra8Unorm`. `src/winit.rs:120-130` fills the rest of
  `SurfaceConfiguration` from `get_default_config`, so `color_space = Auto`,
  which resolves to `Srgb`. There is no hook to choose either.
- `src/renderer.rs:409-414` picks `fs_main_linear_framebuffer` only when
  `format.is_srgb()`, and `fs_main_gamma_framebuffer` otherwise. On
  `Rgba16Float` with scRGB, egui would write gamma values into a linear
  space and the UI would come out wrong. The linear entry point already
  exists and is exactly right for scRGB: sRGB UI colours are valid scRGB
  values in [0, 1].
- Needed (small, upstreamable): a `WgpuConfiguration` option for surface
  format and colour space, and a renderer option "output transfer: gamma or
  linear" decoupled from `is_srgb()`. Until it lands, patch egui-wgpu
  locally (`[patch.crates-io]`), following egui-pop's "work around, then
  upstream" rule. A wider colour space than scRGB (P3, PQ) also needs a 3x3
  matrix and a TF for egui's sRGB colours: a third renderer option, "output
  colour transform".
- Side effect: with linear output, egui blends in linear light instead of
  gamma. Antialiased text edges and translucent panels look slightly
  different. This is the case egui already warns about for sRGB targets
  (https://github.com/emilk/egui/issues/6863).

---

## 3. Path (b): our own `wl_subsurface`, tagged by us

### Bindings and handles

- Crate `wayland-protocols` **0.32.13** (already in Helios's lock through
  winit) ships `staging/color-management/color-management-v1.xml` at
  interface version 3, as `wayland_protocols::wp::color_management::v1::client`,
  behind features **`staging` + `client`**. winit 0.30 already enables
  `staging` (`winit-0.30.13/Cargo.toml:285-288`). winit itself uses no
  colour management, so there is no collision there.
- Parent surface: `raw-window-handle 0.6` `WaylandWindowHandle.surface` and
  `WaylandDisplayHandle.display` (raw pointers). winit links libwayland
  (`wayland-backend` feature `client_system`), so
  `wayland_backend::client::Backend::from_foreign_display(ptr)` plus
  `ObjectId::from_ptr(WlSurface::interface(), ptr)` give a
  `wayland-client` view of winit's connection and surface
  (`wayland-backend-0.3.17/src/sys/client_impl/mod.rs:99, 271`). Use a
  private event queue, as darktable does, so winit's dispatch never sees
  our events.
- Create `wl_surface` + `wl_subsurface` (parent = winit's surface), set an
  **empty input region** so pointer and keys still reach winit/egui, and
  set the position in surface-local logical coordinates.

### Rendering into it with wgpu

- `Instance::create_surface_unsafe(SurfaceTargetUnsafe::RawHandle { .. })`
  with a `WaylandWindowHandle` that points at the subsurface's
  `wl_surface` gives a second wgpu `Surface` on the same `Device`. wgpu and
  Mesa do not care that it has no xdg role (unverified on a machine, but
  the Vulkan WSI only needs a `wl_surface`).
- **Collision:** that second surface is again a Mesa swapchain. If Helios
  configures it with a wgpu colour space, Mesa tags it ("(a) on a
  subsurface": workable, but then (b) brings nothing over (a) except
  leaving the egui surface alone). If Helios wants to tag it itself, it must
  configure `Srgb`, and on Mutter and Hyprland Mesa then creates the colour
  surface first, so our `get_surface` is a fatal `surface_exists` error.
  Whether Mesa tagged a surface cannot be queried; predicting it means
  copying Mesa's private rules. Safe self-tagging therefore means not using
  a WSI swapchain on that surface:
  - SHM buffers, as darktable does: read the viewport texture back (8-bit
    ARGB, or `wl_shm` 16F/10-bit where available) and attach. That costs a
    full readback per changed frame, which is fine for a still image but
    poor for panning at 4K.
  - dmabuf: export a Vulkan image via `VK_EXT_external_memory_dma_buf` and
    `zwp_linux_dmabuf_v1`. That needs `as_hal` image creation with
    external-memory info, plus explicit sync. It is a lot of unsafe code.
- **Interaction with egui:**
  - Stacking: a subsurface *above* the parent would hide egui menus,
    tooltips and overlays drawn over the viewport. darktable puts it
    *below*, punches a transparent hole in the toolkit surface (DEST_OUT),
    and clears the parent's opaque region. Helios would need a transparent
    eframe window (premultiplied alpha), and the viewport callback would
    write alpha 0 over the image rect.
  - Synchronised commits: in sync mode (the default), the subsurface's
    buffer and position apply on the *parent's* commit, so the image, the
    hole and the egui frame land together. The parent's commit is egui's
    Mesa present. A FIFO swapchain on a synchronised subsurface can stall,
    because its frame callbacks depend on the parent committing.
    **Unverified**: use Mailbox/Immediate on the subsurface, or switch to
    desync outside resizes.
  - Scaling: winit uses `wp_fractional_scale_v1` + `wp_viewporter` on the
    parent. The subsurface needs its own viewport (buffer in physical
    pixels, destination in logical units), and an integer logical position
    can leave a one-pixel seam against egui's fractional rect.
  - Resize and hide: recreate or resize on every egui rect change, destroy
    on window unmap. darktable found that a drag icon reuses its parent's
    `wl_surface` and crashed with protocol error 71.
- Cost: this is the ~850-line `wayland.c` of darktable, plus the egui
  transparency changes, Linux-only, for a gain that path (a) gives in a few
  dozen lines.

---

## 4. What darktable PR #22491 does

https://github.com/darktable-org/darktable/pull/22491. Author yikerman,
opened 2026-10-03, **open/unmerged** on 2026-10-12. +5146/-247 lines, heavily
AI-assisted by the author's own account. Tested on Fedora 44, KWin 6.7.5,
Display P3 laptop. Read only, GPLv3; nothing copied here.

- **One subsurface per preview**, darkroom and second window only; the
  lighttable stays as it was (`src/gui/wayland.c`). The subsurface is
  placed *below* the GTK surface, with an empty input region and buffer
  scale set. It uses three ARGB8888 `wl_shm` buffers, and when all three
  are busy it falls back to GTK drawing for that frame. The GTK surface gets
  a DEST_OUT hole where the image is and an empty opaque region. It relies
  on synchronised mode so the hole and the image commit together.
- **The GTK toplevel is tagged too**: every mapped toplevel gets an sRGB
  primaries + gamma22 description with relative intent, so the UI is
  declared, not left to the compositor's default. Non-toplevel windows are
  skipped, because they share their parent's `wl_surface`.
- **Encoding: Rec.2020 primaries + `gamma22`**, fixed for the display's
  lifetime (`src/common/display_transport.c`: an lcms profile with D65,
  BT.2020 primaries and a pure 2.2 curve). The `colorout` module outputs this
  "transport" RGB. Why: Rec.2020 contains any desktop monitor's gamut, so
  the compositor (with its ICC or EDID data) does all display mapping. The
  code comment says gamma 2.2 rather than sRGB because "gamma22 is supported
  by compositors which do not accept the sRGB transfer function" (KWin and
  wlroots do not advertise it, and it is deprecated since protocol v2).
  Preferred-description feedback was considered (Ansel's port uses
  `get_preferred`) and rejected as more complex.
- **Negotiation and fallbacks** (`dt_wayland_color_init`): a private
  `wl_event_queue`, two roundtrips, and a requirement for compositor +
  subcompositor + shm + `parametric` + `bt2020` + `srgb` primaries +
  `gamma22` + **relative intent**. It then creates both descriptions and
  waits up to 2 s for `ready`. On any failure the whole feature is off and
  frozen before image processing starts (BT.709 gamma 2.2 GTK drawing).
  Since Mutter, sway and default Hyprland advertise only perceptual, this
  effectively means **KWin and Weston only** (an inference from the code).
- **Diagnostics**: `darktable-cmstest --wayland` (`src/cmstest/wayland.c`)
  lists the manager's intents, features, TFs and primaries, and for each
  output (`get_output` + `get_image_description` + `get_information`) the
  primaries, TF, luminances, target primaries/luminance, max CLL/FALL and
  ICC file, with a 2 s timeout.
- **What it learned** (thread and code):
  - Most of the ~1200 meaningful lines exist only to keep a Rec.709 GTK UI
    beside a Rec.2020 preview. The author argues that the right design is
    "everything in one wide space".
  - Every widget showing image colours (picker swatches, navigation
    thumbnail, overlays, clipping indicators, gamut warnings) had to be
    converted one by one.
  - The one-colour-surface-per-`wl_surface` rule bites: drag icons crashed
    it.
  - Testing is manual: synthetic wide-gamut TIFFs checked by eye against
    softproof, and a colorimeter as the real proof. The Wayland test is kept
    out of CTest because it is not headless.
  - Reviewers raised packaging concerns (a vendored protocol submodule) and
    a build without Wayland.

---

## 5. Recommendation

### Choice: (a)

1. **One surface, no Wayland code.** wgpu 30 + Mesa ≥ 25.1 already speak
   the protocol, and the egui UI, overlays and colour widgets end up in the
   same declared space. That is the "everything in one space" design
   darktable could not reach on GTK3, and it addresses pitfall 15.
2. **Cross-platform.** The same `SurfaceColorSpace` choice gives scRGB on
   DX12/Vulkan-Windows and EDR on Metal. Path (b) is Wayland-only.
3. (b) has no clean way to present through wgpu without colliding with
   Mesa's tagging (section 3). Its safe form (SHM or dmabuf) is the darktable
   amount of code.

What (a) gives up: Mesa always uses the **perceptual** intent and sets no
luminances, so Helios cannot ask for relative colorimetric. On KWin, how
perceptual differs from relative for SDR content inside the display gamut is
**unverified**. Measure it before claiming colorimetric accuracy. The
display's own description is not known to wgpu on Linux; get it from a
surface feedback probe.

### Minimal first step

1. `DisplayTarget` (section 6) replaces `display_matrix()` and the `encode`
   flag. The viewport shader takes primaries matrix + transfer + white
   scaling as a uniform. Today the only value is "sRGB, gamma-encode,
   clamp", so pixels are identical and existing snapshot tests stay valid.
2. Patch egui-wgpu locally: surface format + colour space selection, and
   output transfer chosen independently of `is_srgb()`. File the issue/PR
   upstream at the same time.
3. On start-up, read `Surface::get_capabilities().format_capabilities`. If
   `Rgba16Float` supports `ExtendedSrgbLinear`, configure scRGB: egui uses
   linear output, and the viewport writes `M(Rec.2020 → BT.709) * rgb`
   with **no clamp below 0** (out-of-sRGB colours become negative), clamped
   only to the target's headroom (1.0 for SDR editing). Otherwise stay on
   today's sRGB path. `DisplayP3` (GNOME, Hyprland) is the second step: it
   needs the egui colour transform.
4. A `helios colour-diag` section (pitfall 14): the wgpu format and colour
   space in use, the capability table, and from a read-only
   `wp_color_management_surface_feedback_v1` on winit's surface (allowed
   next to Mesa's colour surface) the preferred description's primaries,
   TF and target luminances.

### Upstream

- **egui (egui-wgpu / eframe):** configurable surface format and colour
  space; renderer output transfer and optional colour matrix not tied to
  `is_srgb()`. Related: emilk/egui#6863, #7484.
- **wgpu:** `SurfaceColorSpace::Bt2020Linear` (Mesa already lists
  `VK_COLOR_SPACE_BT2020_LINEAR_EXT` wherever bt2020 + ext_linear exist,
  which includes GNOME and sway); maybe `PassThrough` for apps that tag
  surfaces themselves; `display_hdr_info` on Wayland from the protocol.
- **Mesa:** use `gamma22` or `compound_power_2_4` instead of the deprecated
  `srgb` TF, so `DisplayP3` appears on KWin, sway and Weston. This is
  Mesa's call; report it, don't depend on it.
- **winit:** rust-windowing/winit#4131 (colour management getters/events,
  open since 2025-02-20). It would replace our own feedback probe.

### Risks

- Behaviour depends on Mesa version × compositor × driver (NVIDIA
  proprietary unknown). Always fall back to `Srgb`, never fail.
- A `Rgba16Float` swapchain doubles scan-out bandwidth. Some compositors
  may lose direct scan-out for the window (unverified, minor for an editor).
- Linear blending changes egui's look a little. Snapshot tests must pin the
  target.
- scRGB "1.0 = reference white" depends on the compositor's anchoring.
  Values above 1 are HDR headroom that must not be used in SDR editing:
  clamp at 1 until HDR is designed.
- Perceptual intent may compress in-gamut colours on some compositors. Test
  with a measured patch set.

### Headless testing

- **Weston headless** speaks the protocol: Weston ≥ 15 with
  `--backend=headless --renderer=gl` and `weston.ini` `[core]
  color-management=true`. The headless output can be given a profile
  (`[output] color-profile=` / `icc_profile=`, documented in
  `man/weston.ini.man`). Mesa's lavapipe uses the same Wayland WSI, so a
  wgpu Vulkan client under lavapipe goes through the real Mesa tagging path
  (the wgpu PR reports an "SDR fallback checked on lavapipe/Wayland").
  Weston advertises ext_linear + `extended_target_volume`, so scRGB is
  testable there.
- Assertions, cheapest first:
  1. `WAYLAND_DEBUG=1` log: a `wp_image_description_creator_params_v1`
     with the expected primaries/TF and a `set_image_description` on the
     window surface.
  2. Helios's own feedback probe reports the output description.
  3. Pixels: a Weston screenshot (needs the screenshooter allowed, e.g.
     `--debug`, **unverified** for headless + CM) of a patch chart, with
     the output set to a known parametric profile (for example BT.2020
     primaries). Compare against lcms-computed expectations.
- **KWin**: `kwin_wayland --virtual` exists and runs nested. Whether its
  virtual outputs go through colour management (they should, since CM is
  in the compositor's scene) is **unverified**. Try it in wbox as a second
  backend.
- Keep these tests opt-in, as darktable did: they need a compositor, a
  Mesa Vulkan driver and lavapipe in the headless box.

---

## 6. `DisplayTarget` for Helios

Per window/surface (pitfall 2), recomputed on surface or monitor change, and
applied only in the viewport shader and in widgets that show image colours.

```rust
pub struct DisplayTarget {
    /// Primaries + white of the *encoding* the surface declares (not the monitor).
    pub primaries: Primaries,      // Srgb | DisplayP3 | Bt2020 | Custom { r, g, b, w: [f64; 2] }
    /// Transfer the shader applies before writing.
    pub transfer: Transfer,        // Gamma22 | SrgbPiecewise | ExtLinear | Pq | Hlg
    /// Nits the encoding's reference white maps to (protocol `reference_lum`).
    pub sdr_white_nits: f32,       // 80 (sRGB/scRGB), 203 (PQ/HLG), or from feedback
    /// Highest value the target may carry, relative to SDR white (1.0 for SDR).
    pub headroom: f32,             // SDR: 1.0; HDR: target_max_lum / sdr_white
    /// What wgpu was configured with.
    pub surface_format: wgpu::TextureFormat,
    pub surface_colour_space: wgpu::SurfaceColorSpace,
    /// Where the declaration comes from: Mesa WSI, own tagging, untagged/assumed, other OS.
    pub source: TargetSource,
    /// Display description read through surface feedback, for diagnostics.
    pub display: Option<DisplayDescription>, // primaries, tf, min/max/ref lum, target volume
}
```

The shader is `encode(clamp_target(M_target ← Rec.2020 · scene))` with
`M` built from `primaries`. Clamping is per target: `[0, headroom]` for
named SDR spaces, and lower bound -∞ for scRGB, where negatives carry the
gamut.

Mapping to the protocol's parametric description (whether Mesa sends it or
we do):

| DisplayTarget | wgpu colour space (path a) | protocol (path b / what Mesa sends) |
|---|---|---|
| Srgb, Gamma22 or SrgbPiecewise, 80, 1.0, 8-bit | `Srgb` | untagged, or `srgb` + `gamma22` (piecewise → `compound_power_2_4` if advertised) |
| Srgb, ExtLinear, 80, 1.0 (SDR) or >1 (HDR), `Rgba16Float` | `ExtendedSrgbLinear` | `srgb` + `ext_linear` (+ `extended_target_volume`); same as `create_windows_scrgb` except for the reference level |
| DisplayP3, SrgbPiecewise, 80, 1.0 | `DisplayP3` | `display_p3` + `srgb` (Mesa) |
| Bt2020, Gamma22, 80, 1.0 | none in wgpu | `bt2020` + `gamma22` (darktable's choice) |
| Bt2020, ExtLinear, 80, 1.0, `Rgba16Float` | needs the `Bt2020Linear` upstream | `bt2020` + `ext_linear` |
| Bt2020, Pq, 203, peak/203, `Rgb10a2Unorm` or `Rgba16Float` | `Bt2100Pq` | `bt2020` + `st2084_pq`; `set_luminances(min, 10000, 203)`; mastering primaries/luminance + max CLL/FALL via `VK_EXT_hdr_metadata` |
| Bt2020, Hlg, 203, … | `Bt2100Hlg` | `bt2020` + `hlg` |
| Custom primaries | none in wgpu | `set_primaries` (feature) + any TF |

HDR rules for later (pitfall 11):

- **PQ**: scene-linear 1.0 (SDR white) must become 203 cd/m². Encode
  `PQ(Y_nits / 10000)` with `Y_nits = 203 · sdr_white_relative_value`.
  Do *not* map 1.0 to 10000. The protocol's PQ default reference white is
  203, so compositors anchor UI white and image white together. The UI
  (egui) must be drawn at 203 too, so egui's output transform for a PQ
  target is sRGB-decode → BT.709→BT.2020 → ×203/10000 → PQ.
- **HLG**: relative signal with a 1000 cd/m² nominal peak and the BT.2100
  OOTF. The compositor applies it, so do not apply it in Helios.
- **scRGB HDR**: 1.0 = SDR white; `headroom` from the preferred
  description's `target_luminance` max over its reference white (Wayland),
  `display_hdr_info()` (Windows/macOS), or 1.0 when unknown.
- An editing view **clips** at `headroom`; it does not roll off (pitfall 12
  thread). Any tone mapping is an explicit node, never a display side effect.

---

## Sources

- Protocol: https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/color-management/color-management-v1.xml ; https://wayland.app/protocols/color-management-v1 ; crate `wayland-protocols` 0.32.13 (https://crates.io/crates/wayland-protocols)
- wgpu: https://github.com/gfx-rs/wgpu/pull/9658 ; https://github.com/gfx-rs/wgpu/pull/9739 ; crate sources wgpu-types / wgpu-hal 30.0.1
- egui: crate egui-wgpu 0.36.2 (`lib.rs`, `winit.rs`, `renderer.rs`); https://github.com/emilk/egui/issues/6863
- winit: https://github.com/rust-windowing/winit/issues/4131
- Mesa: https://gitlab.freedesktop.org/mesa/mesa/-/blob/main/src/vulkan/wsi/wsi_common_wayland.c ; https://www.phoronix.com/news/Mesa-Vulkan-WSI-HDR-CM ; https://gitlab.freedesktop.org/mesa/mesa/-/merge_requests/28762
- Compositors: links in section 1; https://www.phoronix.com/news/GNOME-wp_color_management_v1 ; https://www.collabora.com/news-and-blog/news-and-events/weston-16-hdr-ready-improved-debugging-and-drm-backend-features.html
- darktable: https://github.com/darktable-org/darktable/pull/22491 ; https://github.com/darktable-org/darktable/issues/19977 ; Ansel's port: https://github.com/aurelienpierreeng/ansel/pull/1389
