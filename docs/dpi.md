# DPI and scaling in egui-pop and Helios (Linux X11/Wayland, Windows)

Status: study, 2026-10-12. Nothing here is decided until ROADMAP.md says so;
items marked *not decided* need the user's call, items marked *[verify]* were
not confirmed against a source or a run.

Versions read: egui / eframe / egui-winit / egui-wgpu / egui_kittest 0.36.2,
epaint 0.36.2, winit 0.30.13, smithay-client-toolkit 0.19/0.20,
wayland-protocols 0.32. Source paths below are relative to each crate's root
(crates.io sources) or to the egui-pop and Helios repositories.

## 0. Vocabulary

| Term | Meaning | Where it comes from |
|---|---|---|
| physical pixel (px) | one pixel of the window's surface | the swapchain size |
| native scale | physical px per logical point, for the window's monitor | winit `Window::scale_factor()`, egui `native_pixels_per_point` |
| interface zoom | the user's "make everything bigger" factor | egui `zoom_factor` (global for the context) |
| `pixels_per_point` (ppp) | native scale × interface zoom, per viewport | egui `Context::pixels_per_point()` |
| point (pt) | egui's layout unit; tokens' `px` and `rem` are points | everything in egui |
| text size | egui-pop's accessibility text scale (fonts.md §3) | `ThemeSpec` `text_scale`, not ppp |
| image zoom | Helios: physical px per image pixel | `view/viewport/zoom` (see §3) |

Three user-visible scales stay separate: **system scale** (the OS), **interface
zoom** (egui-pop setting, scales layout and text), **text size** (egui-pop
setting, scales text and control height only). Image zoom is a fourth, local
to the viewport, and it is the only one measured in physical pixels.

## 1. How each platform gives the scale, and what winit/egui do with it

### 1.1 X11

- **One scale for the whole display.** winit 0.30 reads, in this order
  (`src/platform_impl/linux/x11/util/randr.rs`, `get_output_info`):
  1. `WINIT_X11_SCALE_FACTOR` (a float, or `randr`); the old
     `WINIT_HIDPI_FACTOR` is still read with a deprecation warning; an
     invalid value **panics**;
  2. `Xft/DPI` from XSETTINGS (`x11/xsettings.rs`, value / 1024), then
     `Xft.dpi` from the X resource database; scale = dpi / 96;
  3. only if neither exists: a per-monitor value from RandR's physical size
     (`calc_dpi_factor`: pixels per mm, quantised to 1/12, at least 1,
     ignored above 20 or when the monitor reports 0 mm).
- So with a desktop that sets `Xft.dpi` (GNOME, KDE, Xfce all do), **every
  monitor gets the same scale**. Per-monitor scale on X11 exists only through
  the RandR fallback or `WINIT_X11_SCALE_FACTOR=randr`, which trusts EDID
  millimetres (often wrong: TVs, projectors, KVMs) and yields odd values like
  1.0833.
- **Changes at runtime are followed.** winit watches `_XSETTINGS_SETTINGS`
  and `RESOURCE_MANAGER` (`x11/event_processor.rs`, `process_dpi_change`)
  and sends `ScaleFactorChanged` to every window; in RandR mode a
  `ConfigureNotify` that moves a window to another monitor also does.
- **GNOME on X11**: integer UI scaling only (100 %, 200 %). gsd-xsettings
  publishes `Xft/DPI` already multiplied by the window scale *and* by the
  "large text" `text-scaling-factor` *[verify]*, so on X11 GNOME's text
  scaling becomes an interface scale for winit apps. Ubuntu's patched
  "fractional scaling on X11" renders at the next integer scale and has the
  X server downscale with RandR transforms: the result is blurred and winit
  sees the integer scale *[verify]*.
- **KDE Plasma on X11**: one global scale (fractional allowed) written to
  `Xft.dpi`; Plasma 6 has no per-screen scale on X11.
- Consequence: on X11 a mixed 1x/2x setup is either too small on one screen
  or too big on the other. Helios cannot fix this; it should say so in the
  documentation and offer the interface zoom.

### 1.2 Wayland

- **Integer scale, legacy path.** An output advertises `wl_output.scale`
  (integer). A client learns the scale either from the outputs its surface
  `enter`s or, with `wl_compositor` v6, from
  `wl_surface.preferred_buffer_scale`. The buffer is drawn at that integer
  scale and declared with `wl_surface.set_buffer_scale`. A 1.5 output then
  gets a 2x buffer that the compositor downsamples (soft text).
- **Fractional scale.** `wp_fractional_scale_v1` (staging protocol,
  <https://wayland.app/protocols/fractional-scale-v1>) sends
  `preferred_scale` in 1/120 units per surface; the client draws a buffer of
  `round(logical × scale)` pixels and maps it to the logical size with
  `wp_viewporter` (<https://wayland.app/protocols/viewporter>), buffer scale
  left at 1. Supported by mutter (GNOME, with fractional scaling enabled),
  KWin (Plasma 5.27+), sway, Hyprland, and others; weston has no fractional
  scale *[verify]*.
- **winit 0.30 supports it.** `src/platform_impl/linux/wayland/types/wp_fractional_scaling.rs`
  and `wp_viewporter.rs`; when the manager exists, the legacy integer
  events are ignored (`wayland/state.rs`, "Don't update the scaling factor,
  when legacy method is used"); physical sizes are
  `round(logical × scale)` (`logical_to_physical_rounded`). Without the
  protocol, winit sets the integer buffer scale (`window/state.rs`).
- **Per surface, chosen by the compositor.** A window spanning two outputs
  gets one preferred scale, compositor's choice (usually the output with the
  largest share, or the larger scale). Moving a window to another output
  emits a new `preferred_scale` and winit sends `ScaleFactorChanged`.
- **Values are multiples of 1/120.** 1.25 = 150/120 and 1.5 = 180/120 are
  exact; 1.333 is 160/120, and buffer sizes round, so the logical window
  size in points is not an integer: never assume `size_px / ppp` is.
- **No global positions.** A client cannot choose its output or position
  (except fullscreen on an output), so "restore an auxiliary window on its
  2x monitor" is not possible on Wayland; the compositor places it, and its
  scale is known only after the first configure (pitfall §11 in
  `docs/darktable-pitfalls/ui-platforms.md`).
- **XWayland** (if a user forces `WAYLAND_DISPLAY=` unset / `WINIT_UNIX_BACKEND`):
  GNOME 47+ can let X clients scale themselves ("xwayland-native-scaling"),
  KDE has "apply scaling themselves" which sets `Xft.dpi`; otherwise the
  compositor upscales a 1x X client (blurry). Helios runs native Wayland;
  keep it so.

### 1.3 Windows

- **DPI awareness modes** (Microsoft, "High DPI desktop application
  development on Windows",
  <https://learn.microsoft.com/windows/win32/hidpi/high-dpi-desktop-application-development-on-windows>):
  unaware (bitmap-stretched by DWM, blurry), system aware (one DPI read at
  start, stretched on other monitors), per-monitor (v1, Windows 8.1),
  **per-monitor v2** (Windows 10 1703+: non-client area, child windows,
  common dialogs and theme bitmaps scale automatically, and
  `WM_DPICHANGED` reaches every top-level window).
- **winit 0.30.13 asks for v2 itself.** `src/platform_impl/windows/dpi.rs`,
  `become_dpi_aware`, called when the event loop is built: tries
  `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`, then `PER_MONITOR_AWARE`,
  then `SetProcessDpiAwareness(PROCESS_PER_MONITOR_DPI_AWARE)`, then
  `SetProcessDPIAware`; it also calls `EnableNonClientDpiScaling` per window
  (useful for v1 only). This settles the "[verify v2 on winit 0.30.13]" in
  `docs/windows-port.md`: yes, v2.
- **The manifest is still recommended.** Microsoft: set the default awareness
  in the manifest, not by API ("Setting the default DPI awareness for a
  process", <https://learn.microsoft.com/windows/win32/hidpi/setting-the-default-dpi-awareness-for-a-process>).
  The API must run before any window exists in the process, and only once;
  if the manifest already set it the call fails with `E_ACCESSDENIED`, which
  is harmless (winit ignores the result). The manifest covers windows made
  before winit (a crash dialog, a native file dialog opened early), and
  what Explorer and Task Manager report. Snippet for the Windows GUI
  manifest work:

  ```xml
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  ```

  A user can still override it per executable (Properties → Compatibility →
  "Override high DPI scaling behaviour"); nothing to do but recognise the
  symptom (blurry or wrong-size window) in bug reports, and have the debug
  bridge report the awareness context.
- **Scale values**: Settings offers 100, 125, 150, 175, 200, 225, 250, 300,
  350 % (depending on the monitor), plus a custom 100–500 %. winit:
  `scale_factor = dpi / 96` (120 → 1.25, 144 → 1.5, 168 → 1.75, 192 → 2).
- **Changing monitor**: `WM_DPICHANGED`
  (<https://learn.microsoft.com/windows/win32/hidpi/wm-dpichanged>) carries
  the new DPI and a suggested window rectangle; a window's DPI is that of the
  monitor holding most of it, so it flips mid-drag when the larger part
  crosses. winit (`windows/event_loop.rs`, `WM_DPICHANGED`) sends
  `ScaleFactorChanged { scale_factor, inner_size_writer }` and applies the
  suggested rectangle (keeping the logical size), except when maximised or
  fullscreen. winit does not handle `WM_GETDPISCALEDSIZE`, so it cannot
  correct the size before Windows moves the window; near a boundary with
  different scales a window can bounce between DPIs *[verify]*.
- **Text size** (Accessibility → Text size, 100–225 %) is separate from the
  scale; Win32 apps must read it themselves (`UISettings.TextScaleFactor`).
  winit does not. It maps to egui-pop's text size setting (a default, *not
  decided*).

### 1.4 What winit reports, what egui and eframe do with it

- winit: `Window::scale_factor()` and
  `WindowEvent::ScaleFactorChanged { scale_factor, inner_size_writer }`
  (<https://docs.rs/winit/0.30.13/winit/dpi/index.html> has the per-platform
  table). Sizes come in `PhysicalSize`; the writer lets the app pick the new
  physical size (default: same logical size).
- egui-winit (`src/lib.rs`): on `ScaleFactorChanged` it stores
  `ViewportInfo::native_pixels_per_point` for that viewport; each frame
  `pixels_per_point = zoom_factor × native_pixels_per_point`.
- egui (`src/context.rs`): `pixels_per_point()` is **the current viewport's**
  (inside `show_viewport_*` it is the auxiliary window's). `zoom_factor` is
  one value for the whole context (all windows), set with
  `set_zoom_factor`, applied at the next pass, repainting every viewport.
  `set_pixels_per_point` is a `set_zoom_factor` in disguise.
- `Options::zoom_with_keyboard` is **true by default**: Ctrl/Cmd + `+`, `=`,
  `-`, `0` change `zoom_factor` (`src/gui_zoom.rs`). egui-pop turns it off
  (2026-10-12) and owns the interface zoom (`Ctrl+Alt+=`, `Ctrl+Alt+-`,
  `Ctrl+Alt+0`, rebindable), so Helios's image zoom can take Ctrl+= and
  Ctrl+- (`docs/shortcuts-research.md`) without both firing.
- Fonts: one `Fonts` per context; `pixels_per_point` is part of the glyph
  key, so glyphs are rasterised per ppp in one shared atlas. A scale change
  costs a lazy re-rasterisation of the glyphs drawn next (a slow frame or
  two), not a font reload; two windows at two scales keep two glyph sets;
  above 80 % fill the atlas is thrown away and rebuilt (egui-pop
  `docs/fonts.md`, "Hinting, sub-pixel, HiDPI, memory").
- Tessellation (epaint `src/tessellator.rs`, `TessellationOptions`):
  `round_text_to_pixels`, `round_line_segments_to_pixels` (axis-aligned
  segments) and `round_rects_to_pixels` are on by default; feathering is
  1 physical px. Layout itself rounds to 1/32 point (`emath::GuiRounding`),
  not to the pixel grid.
- eframe: window sizes and positions it saves are in points, converted with
  `zoom_factor × monitor.scale_factor()` (`src/native/epi_integration.rs`).

## 2. What egui-pop must handle

egui-pop today has no scale code at all (no use of `pixels_per_point`,
`zoom_factor` or `native_pixels_per_point` in `crates/`). That is mostly
right, since egui does the per-viewport work, but these points need owning:

1. **Per-window scale is egui's, and widgets must ask the right context.**
   Rule: read `ui.ctx().pixels_per_point()` (or `ui.pixels_per_point()`)
   where you paint, never a value cached by an app-level object. Helios
   breaks it today (§3). A lint is not possible; a test with an auxiliary
   viewport at another scale is (§5).
2. **Reacting to a scale change.** Layout is in points, so nothing moves;
   only rasterisation changes. Costs to keep small: the glyph re-raster
   (few text sizes, few weights, as fonts.md already requires) and any
   texture sized in physical pixels (thumbnails, icons rasterised from SVG,
   the Helios viewport). egui-pop should give apps a cheap signal:
   `egui_pop_core::scale::changed(ctx) -> Option<(f32, f32)>` (old, new ppp
   for this viewport, kept in temp memory), so caches keyed by physical size
   are invalidated once, not compared every frame.
3. **Interface zoom, owned by egui-pop.**
   - Turn off `zoom_with_keyboard` at install; expose commands
     `view.interface.zoom_in/out/reset` bound by the keymap like any other
     command (key choice *not decided*; Ctrl+= stays free for content zoom).
   - Quantised steps (for example 67, 75, 80, 90, 100, 110, 125, 150, 175,
     200 %), applied on release, never per slider frame: each distinct ppp
     fills the atlas with a new glyph set.
   - Persist it in egui-pop settings, not egui's memory; one value for all
     windows (it is a person's preference, the monitor part is native).
   - Interface zoom × text size × native scale multiply; fonts.md's
     `pt = round_to_half(rem × base × text_scale)` stays in points, and ppp
     does the rest.
4. **Density tokens** (`density-comfortable`, `density-compact`) stay in
   points and are independent of scale. A "compact on HiDPI" default is a
   profile concern (Helios already has the `hidpi` selector), not a token
   rule.
5. **Crisp 1-px lines at 1.25, 1.5, 1.75.** A 1 pt stroke is 1.25 px at
   1.25: egui feathers it over two pixel columns, a grey line instead of a
   sharp one. Rule, the one CSS uses for border widths ("snap as a border
   width", CSS Values 4): a width `w` pt becomes `w × ppp` px, floored to a
   whole pixel when ≥ 1 px, raised to 1 px when below, then divided back by
   ppp. Provide `scale::stroke(ctx, w_pt) -> f32` and `scale::hairline(ctx)`
   (`1 / ppp`), and use them in egui-pop's theme install
   (`widgets.*.bg_stroke`, separators, `border.*`) and in painted widgets.
   Positions: draw axis-aligned strokes on pixel centres
   (`Painter::round_to_pixel_center`, `GuiRounding::round_to_pixel_center`),
   fills on pixel edges (`round_to_pixels`). egui does both for rects and
   axis-aligned segments when the default tessellation options are kept;
   egui-pop must not turn them off, and its own polylines (painted icons,
   chevrons, slider ticks) must snap their end points.
6. **Icon sizes.** Make every `size.icon.*` and `size.control.height` token a
   multiple of 4 pt: 4 × 1.25 = 5, 4 × 1.75 = 7, so such sizes are whole
   pixels at every quarter scale. Phosphor glyphs are rasterised per ppp like
   text and stay sharp; egui-pop's painted icons (`egui-pop-core/src/icon.rs`,
   stroke `rect.width() / 11` with a 1.25 floor) should snap their rect to
   the pixel grid and their stroke with `scale::stroke`. A token test can
   check the multiple-of-4 rule.
7. **Images and textures.** A bitmap shown at `s` pt should be uploaded at
   `ceil(s × ppp)` px for that window, or with mipmaps
   (`TextureOptions { mipmap_mode: Some(Linear), .. }`) when one source
   serves several scales. Linear filtering without mipmaps aliases when
   downscaling by more than 2× (Helios's 64 px logo drawn at 24 pt is a
   2.7× reduction at 1x).
8. **Debug bridge: a `screen` core collector.** Snapshot per viewport: id,
   title, native ppp, zoom factor, ppp, inner and outer rect in points and
   in pixels, monitor size, maximised/fullscreen, the font atlas size and
   fill (already proposed for the `fonts` collector), and on Windows the
   process DPI awareness. Actions: `set_interface_zoom { factor }`
   (mutates). Agents then check "which window is at which scale" without a
   screenshot.
9. **Window state.** `egui_pop_api::window::WindowState` keeps sizes in
   points. Also store the native scale at save time; when restoring onto a
   monitor of another scale, points still give the same apparent size, but
   a saved position must be checked against the screens (the existing
   eframe gap in `docs/upstream.md`).

## 3. What the Helios image viewport must do

Reference: `docs/darktable-pitfalls/ui-platforms.md` §10 ("define 100% once:
one image pixel per physical screen pixel"), darktable #10839 and PR #11179.

### 3.1 What the code did (before 2026-10-12)

Done since: the viewport runs on egui-pop's (`egui-pop-viewport`), which
does all of §3.2 (a physical zoom stored in `view/viewport/zoom`, 1.0 for
100 %; whole-pixel offsets; the ladder and the wheel's detent at 100 %;
one mapping for drawing and the readout; half-float tiles within the
device's texture limit). What follows is kept as the record of why.

- `viewport.rs` keeps `View::zoom` as **points on screen per image pixel**
  and stores it as `view/viewport/zoom`, displayed ×100 with a `%` unit.
- `viewport.zoom.100` ("Image: actual pixels (100 %)") sets
  `zoom = 1 / shell.pixels_per_point` (`shell.rs`, `Zoom::Actual`). At ppp
  2 the 1:1 view is right but the value shown is **50 %**; at ppp 1.25 it
  shows 80 %.
- `shell.pixels_per_point` is copied from the main window's context at the
  start of `Shell::ui`. If the viewport panel is detached into an auxiliary
  window on another monitor, "100 %" uses the wrong monitor's scale.
  (Fixed since: the zoom is egui-pop's, in physical pixels of the window
  that draws it; `tests/scale.rs` checks 100 % in a detached 2x window.
  `shell.pixels_per_point` is the main window's and only crops its
  pictures, which refuse a target drawn in a detached window.) (The
  viewport's own render requests use `ctx.pixels_per_point()` of the window
  that draws it, which is right; so do `Paint::prepare` and the shader,
  which work in physical pixels through `ScreenDescriptor`.)
- `zoom = 1/ppp` in f64, multiplied back by `ppp` as f32 in the uniform,
  is not exactly 1.0; combined with a half-pixel image offset (§3.2) this
  can repeat or skip a column now and then.
- Exact renders are requested at `scale = zoom × ppp` (physical), proxies
  at a power of two of the fit scale: already physical, good.
- Layers are `rgba32float`: a 3840×2160 physical view at 1:1 is 133 MB per
  layer, four times the 1x cost.

### 3.2 Target

1. **Store the zoom in physical pixels per image pixel.** 100 % means
   `zoom_px = 1`. Points per image pixel = `zoom_px / ppp`, computed each
   frame from the window that draws the viewport. A window moved to another
   monitor keeps "100 %" at 1:1 (the image changes apparent size, which is
   what 1:1 means). "Fit" stays derived from the view rect. Label the
   control "100 % (1 image px = 1 screen px)" in its tooltip. *Not decided*:
   whether to also offer a "100 % of points" (darktable's current
   behaviour); recommendation: no, offer "fit" and the ladder instead.
2. **1:1 at fractional scales.** In the shader an output pixel centre
   `n + 0.5` maps to image `q = (n + 0.5 − c_px) / zoom_px + centre_img`;
   for a clean 1:1 (and for integer zooms 2, 3, 4 and 1/2, 1/3...) the
   offset `c_px − centre_img × zoom_px` must be a whole number of physical
   pixels, otherwise samples land on texel edges and float error picks
   either side. Snap: compute the view rect in physical pixels
   (`rect × ppp`, rounded), then round the image centre so the offset is
   integral at integer or 1/n zooms. Keep `zoom_px` exactly 1.0 for 100 %
   (no `1/ppp × ppp` round trip).
3. **Zoom steps.** A ladder in physical terms, whole ratios first:
   6.25, 8.33, 12.5, 16.7, 25, 33.3, 50, 66.7, 100, 200, 300, 400, 800,
   1600, 3200 %. `viewport.zoom.in/out` walk the ladder (today ×1.25 and
   ×0.8); the wheel stays continuous but snaps to 100 % when it crosses it
   (a detent), so a user can always come back to exact pixels. Above
   100 %, nearest-neighbour (the shader's `textureLoad`) is the right look
   for pixel peeping; below, the engine renders the exact layer at
   `zoom_px`, drawn 1 texel per physical pixel.
4. **Pixel readout.** One Rust function for both the readout and the tests:
   `image_px(pointer_pt) = floor(centre_img + (pointer_pt − rect_centre_pt) × ppp / zoom_px)`
   with the same snapped offset as the shader. egui pointer positions are
   physical / ppp, so this is exact. A test walks every physical column of a
   1:1 view and checks the readout equals the texel the shader drew.
5. **Display transform and textures at physical resolution.** Already so
   (the paint callback works in `ScreenDescriptor` pixels). Changes: use
   `rgba16float` for the display layers (half the memory, enough precision
   after the pipeline; keep f32 for `viewport.scene` analysis), and tile a
   layer that exceeds the device's `max_texture_dimension_2d` (a wide
   window spanning monitors at 2x). Re-request the exact render on a scale
   change (the request key already contains the scale).
6. **Tests and snapshots** crop with `rect × ppp` (`snapshot.rs`); keep that,
   but take ppp from the viewport's own window.

## 4. Own window frame

`crates/helios-gui/src/frame.rs`: title bar 32 pt, buttons 46 pt, resize
grip 5 pt inside the client area, `ViewportCommand::StartDrag` and
`BeginResize`.

- **Sizes in points scale with the window**: 5 pt is 5 px at 1x, 10 px at
  2x. Below 1x interface zoom it shrinks; make the grip
  `max(5 pt, 6 px / ppp)` so it never falls under 6 physical pixels.
  Windows' own resizing border is about 8 px at 100 % (padded border plus
  frame) *[verify]*; native frames put it outside the visible window,
  ours sits inside and overlaps the content edge (a scrollbar at the right
  edge loses its outer 5 pt). Inset scrollable areas by the grip, or hit
  the grip only when no widget under the pointer wants the drag.
- **Scale change mid-drag**: `StartDrag` hands the move to the system
  (`_NET_WM_MOVERESIZE` on X11, `xdg_toplevel.move` on Wayland,
  `WM_NCLBUTTONDOWN(HTCAPTION)` on Windows), so the window keeps its size in
  points while its pixel size changes; nothing in the frame stores pixels.
- **Icons and the logo**: the painted buttons use 1 pt strokes (blurred at
  1.25 and 1.5: use `scale::stroke`); the logo texture needs mipmaps or a
  physical-size upload (§2.7).
- **Windows Snap Layouts** (Windows 11): the flyout appears only when
  `WM_NCHITTEST` returns `HTMAXBUTTON` over the maximise button
  (<https://learn.microsoft.com/windows/apps/desktop/modernize/ui/apply-snap-layout-menu>).
  winit 0.30 has no hit-test hook (it handles `WM_NCCALCSIZE` for
  undecorated windows but not `WM_NCHITTEST`). Options, as in
  `docs/windows-port.md` §5: (a) the native title bar on Windows
  (`draw_frame(false)`), (b) subclass the window procedure
  (`SetWindowSubclass` on the `HWND` from `raw-window-handle`) and answer
  `WM_NCHITTEST` from rectangles the frame publishes each frame, in
  **physical client pixels** (`rect × ppp`, rounded), giving `HTCAPTION`
  on the bar (native drag, double click, right-click system menu),
  `HTMAXBUTTON` on maximise (the click then arrives as `WM_NCLBUTTONUP`, so
  maximise must be triggered from there, not from egui), `HTLEFT`...`HTBOTTOMRIGHT`
  on the grips; (c) upstream a hook in winit. (b) needs `unsafe`, so it
  lives in Helios's platform crate (egui-pop forbids unsafe code), with the
  rectangles exchanged through an atomic or a mutex. Recommendation: (a)
  by default on Windows until (b) exists; *not decided*.

## 5. Tests

- **Forcing a scale in kittest**: `Harness::builder().with_pixels_per_point(s)`
  sets the root viewport's `native_pixels_per_point` (egui_kittest
  `src/lib.rs`). The render follows: kittest's wgpu renderer sizes the image
  `content_rect × ppp`.
- **Simulating a monitor change**: `Harness::set_pixels_per_point` does
  **not** do it: it calls `ctx.set_pixels_per_point`, which changes
  `zoom_factor`. To simulate `ScaleFactorChanged`, write
  `harness.input_mut().viewports.get_mut(&ViewportId::ROOT).native_pixels_per_point = Some(s)`
  and step (keeping `screen_rect` in points, as winit keeps the logical
  size) *[verify that the field survives between steps]*. egui-pop's test
  kit should wrap it: `egui_pop_test::set_native_scale(&mut harness, s)`.
- **Matrix**: a helper running one test body at 1, 1.25, 1.5 and 2.
  Recommended split, to keep the snapshot set small:
  - **full snapshots at 1 and 2** (integer scales are stable across GPUs),
    named `name@2x.png`;
  - **scale-independent checks at all four**: widget rects in points (from
    AccessKit or `name_rect`) equal across scales within 1/32 pt; no text
    clipped (label rect inside its widget); a separator or 1 pt border is
    exactly one opaque physical column (read the pixels at the expected
    place: crisp-line check); text-size and interface-zoom steps keep the
    atlas below a fill limit.
- **Mixed DPI**: kittest embeds immediate viewports as windows inside the
  root (`embed_viewports` is true), so two windows at two scales cannot be
  rendered there by kittest alone. Since 2026-10-12,
  `helios_gui::headless::harness_with_windows` turns embedding off and
  runs each detached panel's window in a pass of its own, as eframe does,
  on a screen of the scale the test sets (`Windows::set_scale`), and
  renders it on the harness's device (`Windows::render`);
  `tests/scale.rs` checks a viewport detached to a 2x screen beside a
  main window at 1x (scales, fit, 100 %, whole-pixel strokes, pictures).
  Cover the rest in layers:
  - unit level: the viewport and frame code takes ppp as an argument;
    tests call it at two scales and assert tile requests, "100 %" and grips;
  - a single-window scenario: Helios at 1x, open an image at 100 %, switch
    the root to 1.5 with `set_native_scale`, assert the stored zoom is still
    1.0, the exact render is requested at the new physical size, layout
    rects in points did not change, and the readout maps 1:1;
  - real runs in wbox (headless, never on the desktop): X11 Xvfb with
    `WINIT_X11_SCALE_FACTOR=1.25` (one scale only); for a real mixed setup,
    a headless sway with two outputs (`output HEADLESS-1 scale 1`,
    `output HEADLESS-2 scale 1.5`) and `swaymsg` moving the auxiliary
    window, checked through the debug bridge's `screen` collector
    *[verify sway's headless backend and fractional-scale support in the
    wbox image]*;
  - Windows: manual or UI-automation runs on a two-monitor VM per release
    (`docs/windows-port.md` §1.7 already says so).

## 6. Upstream gaps, local workarounds, plan

### 6.1 Gaps (to add to egui-pop `docs/upstream.md` as they are worked around)

| Gap | Upstream | Workaround |
|---|---|---|
| kittest cannot change the native scale mid-run (`set_pixels_per_point` changes zoom) | egui_kittest, issue to file | `set_native_scale` through `input_mut()` |
| kittest renders one viewport only | egui_kittest | `harness_with_windows` (Helios; for egui-pop's test kit once settled), wbox runs |
| the viewport overlay's strokes (selected shape, handles: 1.5 and 3 pt) are not snapped | egui-pop | none yet: blurred at 1.25 and 1.5, the handles' outline at 1 |
| `zoom_with_keyboard` on by default, keys collide with content zoom | config, not a bug | turn off at install, egui-pop commands |
| no helper to snap stroke widths at fractional ppp | egui, could be proposed | `egui_pop_core::scale::stroke` |
| no `WM_NCHITTEST` hook (Snap Layouts) | winit, issue to find or file | native frame on Windows, or subclassing in Helios |
| no `WM_GETDPISCALEDSIZE` handling | winit | none; watch for DPI bounce at monitor edges |
| X11: one scale for all monitors (Xft.dpi) | by design | document; interface zoom |
| screens' rectangles and scales unknown at window placement | eframe (already listed) | `WindowState::repair` |

### 6.2 egui-pop commits (one purpose each, tests in each)

1. `scale` module in `egui-pop-core`: `Scale::of(ctx)` (native, zoom, ppp
   for the current viewport), `stroke`, `hairline`, `changed`. Unit tests at
   1, 1.25, 1.5, 1.75, 2.
2. Interface zoom: turn off `zoom_with_keyboard`, add the quantised setting
   and the `view.interface.*` commands; keymap test that no default binding
   remains on Ctrl+=/−/0 from egui.
3. Theme: strokes and borders through `scale::stroke`; painted icons snap;
   token test "icon and control sizes are multiples of 4 pt".
4. Test kit: `harness_at(scale)`, `set_native_scale`, the scale matrix, the
   crisp-line and points-equality checks; gallery snapshots at 2x for the
   overview pages; upstream.md entries.
5. Debug bridge `screen` collector (and the `fonts` collector's atlas
   fields if not landed).
6. `WindowState` records the save-time scale; docs: a "Scaling" section
   (the vocabulary of §0) in egui-pop's docs, linked from tokens.md and
   fonts.md.

### 6.3 Helios commits

1. Viewport: zoom in physical px per image px, 100 % = exactly 1.0, ppp from
   the drawing window (drop the shell's cached value for this), offset
   snapping; tests at four scales with a unique-colour test image checking
   1:1. The stored value changes meaning: saved views at 1x are unchanged,
   at 2x they halve once (acceptable before release; note it in the
   changelog fragment).
2. Zoom ladder, wheel detent at 100 %, readout function shared with the
   tests.
3. `rgba16float` display layers and tiling past the texture limit.
4. Frame: grip minimum in physical px, snapped strokes, logo mipmaps.
5. Windows: the manifest (PerMonitorV2 + longPathAware) with the Windows GUI
   basics work; frame choice per §4 (*not decided*).
6. wbox mixed-DPI scenario (sway, two outputs) once the debug bridge reports
   scale per window.

### 6.4 Relation to existing tracker items

- The **input and platforms** item already lists "per-window pixel ratio":
  Helios commits 1 and 6 and egui-pop commits 1, 4 and 5 implement it; this
  study can be its design reference.
- The **Windows GUI basics** item (no console, application manifest, focus
  mode in-process) takes the manifest snippet of §1.3; DPI awareness needs
  no code there since winit already requests v2.
- `docs/windows-port.md` §5 "DPI" row: replace "[verify v2 on winit
  0.30.13]" by the finding (§1.3), and point to this study.

## 7. Risks

- **Glyph atlas churn**: mixed DPI doubles the glyph set, a continuous
  interface-zoom or text-size slider multiplies it; atlas rebuilds give
  visible hitches. Mitigation: quantised steps applied on release, few
  sizes.
- **1:1 correctness depends on rounding**: a half-pixel offset or a zoom of
  0.99999994 duplicates columns, the exact thing pixel peepers notice.
  Mitigation: physical zoom, snapped offsets, a per-column test.
- **GPU memory at 2x**: four times the texels for the viewport layers.
  Mitigation: half floats, tiling.
- **Snap Layouts needs unsafe Win32 subclassing**, the riskiest code in the
  plan, and untestable headless.
- **Platform limits we cannot fix**: X11 has one scale; Wayland compositors
  without fractional scale give blurred 1.5x; Wayland cannot place a window
  on a chosen output.
- **Mixed-DPI testing** relies on wbox with a compositor that supports two
  outputs at different scales, and on manual Windows checks.
