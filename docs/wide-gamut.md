# Wide gamut

Helios edits in linear Rec.2020. What the viewport shows of it depends on
the encoding the image is drawn in: sRGB clips every colour outside the
sRGB gamut, a wider encoding keeps them. The interface itself (egui) is
always sRGB; only the image asks for more.

## How it is wired

- **The display transform** (`helios_gui::viewport::HeliosDisplay`) takes
  the target egui-pop draws for and converts linear Rec.2020 to its
  primaries, clamps to its range and encodes with its transfer:

  | Target | Clamp | Out-of-sRGB colours |
  |---|---|---|
  | sRGB (piecewise curve), SDR | `[0, 1]` | clipped |
  | Rec.2020, gamma 2.2, SDR | `[0, 1]` | kept |
  | extended sRGB or scRGB, half floats | `[-headroom, headroom]` | kept, as negatives |
  | Display P3 | `[0, 1]` | kept within P3 |

  Headless tests in `viewport.rs` check a green outside sRGB per target.
- **The Wide-gamut preview setting** (Display, off by default): on, the
  viewport's window asks egui-pop for Rec.2020 primaries with a 2.2 power
  (`egui_pop::viewport::display::want`), the encoding a Wayland compositor
  maps to the screen with its own profile (`bt2020` + `gamma22`, the
  choice darktable's Wayland work made too).
- **The image's own surface** (egui-pop's `image_surface`, Wayland): the
  main window is built transparent and cleared opaque, so it looks as it
  always did. The first frame the setting is on, once the viewport's GPU
  path is installed, the shell installs the image surface
  (`helios_gui::display::ImageSurfaceSwitch`); egui-pop then draws the
  main window's image into a subsurface below the window, seen through a
  hole in the interface, and negotiates its encoding with the compositor:

  | Route | Where | The image |
  |---|---|---|
  | `tagged` | KWin 6.3 and later, sway, Weston: the compositor does not announce the sRGB transfer | tagged Rec.2020 + gamma 2.2 by egui-pop through `wp_color_management_v1` |
  | `wsi` | GNOME 48 and later, Hyprland: the compositor announces sRGB with the sRGB transfer, so the GPU driver holds the surface's colour | presented in a colour space the driver declares (Display P3 with Mesa) |
  | `srgb` | X11, a compositor without colour management, the CPU path, a detached viewport window | on the window's surface, sRGB, with the reason |

  The viewport draws for the negotiated target, and the display transform
  encodes for it. When the setting goes off, the image surface is taken
  down: the image is back on the window at once, and egui-pop drops the
  subsurfaces as the viewport wishes sRGB again.
- **Diagnostics**: `helios doctor display` ([command line](cli.md#doctor))
  prints the session, what the compositor's `wp_color_management_v1`
  announces, and the route it predicts from that (`viewport`: `route`,
  `encoding`, `reason`). In the running desktop, the debug bridge's
  `helios.display` collector shows the same, what the window resolved
  (wanted, surface, drawn, reason) and the `route` it actually
  negotiated; egui-pop's `viewport` collector shows the image surface in
  detail (the compositor's announcements and preferred description, each
  subsurface's place, tag and configuration).
- **Tests**: headless, with fake compositors through egui-pop's
  negotiation (`image_surface::negotiate`): with the setting off the
  target stays sRGB and no surface is installed; on, the target follows
  the route (tagged on a KWin-like compositor, Display P3 on a GNOME-like
  one, sRGB without colour management, each with its reason).

## What works where

| Platform | The image |
|---|---|
| Wayland with `wp_color_management_v1`: KWin 6.3+, sway, Weston 15+ | Rec.2020 + gamma 2.2 (`tagged`) |
| Wayland with `wp_color_management_v1`: GNOME 48+, Hyprland 0.48+ | Display P3, declared by the driver (`wsi`) |
| Wayland without the protocol, X11 | sRGB: the compositor cannot be told |
| no GPU path (the viewport on the CPU) | sRGB: the CPU path draws 8-bit sRGB textures |
| a viewport detached to its own window | sRGB: the image surface serves the main window only |
| macOS, Windows | out of scope for now (an extended-range colour space with Metal, HDR swapchains on DX12); the transform already encodes extended sRGB and scRGB |

Turning the setting on is safe: whatever the window cannot carry falls
back to the sRGB it shows with the setting off. `EGUI_POP_IMAGE_SURFACE=off`
turns the image surface off altogether, `=no-tag` keeps the driver's
colour spaces only (for a driver holding surfaces' colour beyond Mesa's
rule).

## Checking it on a screen

Automation has no colour-managed Wayland session, so the owner checks it
by hand:

1. **The session.** A Wayland session on KWin 6.3 or later (Plasma 6.3),
   the reference for the `tagged` route, on a wide-gamut screen (Display
   P3 or wider) with its colour profile or its wide colour gamut mode
   set in the display settings; Mesa's Vulkan driver.
2. **What the compositor offers.** `helios doctor display` should print
   `"session": "wayland"`, `color_management` with `parametric`, the
   primaries `bt2020` and the transfer `gamma22`, no `srgb` transfer, and
   `"viewport": { "route": "tagged", ... }`.
3. **The setting.** Open Helios on an image, turn on Display > Wide-gamut
   preview. Nothing else in the window may change: same colours in the
   interface, no seam, offset or black border around the image; overlays,
   the before/after split, menus and tooltips drawn over it. Pan, zoom
   and resize: the image follows in the same frame as the interface.
4. **The route.** The debug bridge's `helios.display` collector gives
   `"route": { "route": "tagged", "encoding": "BT.2020 primaries, gamma 2.2 transfer, SDR", "reason": null }`
   and `resolution.faithful: true`. `WAYLAND_DEBUG=1 helios-gui 2>&1 | grep -E 'wl_subsurface|set_primaries_named|set_tf_named|set_image_description'`
   shows one subsurface, `set_primaries_named(6)` (BT.2020),
   `set_tf_named(2)` (gamma 2.2) and `set_image_description` on the
   subsurface.
5. **A BT.2020 test chart.** Open an image holding colours outside sRGB
   but inside the screen's gamut: a chart of saturated patches in
   Rec.2020 (a ramp of pure Rec.2020 greens, and P3 red, green and blue
   next to their sRGB counterparts). With the setting off, the patches
   outside sRGB clip: a P3 green and its sRGB neighbour look the same.
   With it on, they come apart; patches inside sRGB look the same in
   both cases.
6. **Back off.** Turn the setting off: the image goes back to sRGB in
   the next frame (the out-of-sRGB patches clip again) and the collector
   says `"route": "srgb"` with "the Wide-gamut preview setting is off".

On GNOME 48 or later the same steps give `wsi` and Display P3: a P3
chart comes apart from sRGB, BT.2020 greens beyond P3 still clip.

## What is missing

- **The image surface's connection stays until exit.** Taken down, the
  image surface drops its subsurfaces, but egui-pop has no call to
  uninstall it, so its connection to the compositor (no surface on
  screen) lives on until the window closes, and serves again when the
  setting comes back on.
- **The main window only.** A viewport detached to a window of its own
  stays sRGB.
- **HDR.** The image surface can carry PQ, but Helios asks for SDR
  Rec.2020; the whole window in HDR waits for egui-wgpu to take a colour
  space for the window's swapchain.
- **The output's description is not used.** egui-pop reads the
  compositor's preferred description for the window (its gamut and
  luminances); Helios does not adapt the wish to it yet.

The research behind the choice of paths (the swapchain colour space
through Mesa, or a tagged subsurface) is in
[wayland-colour.md](wayland-colour.md).
