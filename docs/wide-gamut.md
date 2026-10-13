# Wide gamut

Helios edits in linear Rec.2020. What the viewport shows of it depends on
the encoding the window's surface is drawn in: sRGB clips every colour
outside the sRGB gamut, a wider encoding keeps them. The interface itself
(egui) is always sRGB; only the image asks for more.

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
  choice darktable's Wayland work made too). egui-pop draws it when the
  window's surface carries it, and sRGB otherwise, with the reason.
- **Diagnostics**: `helios doctor display` ([command line](cli.md#doctor))
  prints the session, what the compositor's `wp_color_management_v1`
  announces, and what the image is drawn in there. In the running
  desktop, the debug bridge's `helios.display` collector shows the same
  plus what the window resolved (wanted, surface, drawn, reason), and
  egui-pop's `viewport` collector shows it per viewport.

## What works where

| Platform | Today | What it takes |
|---|---|---|
| Wayland with `wp_color_management_v1` (KWin 6.3, GNOME 48, sway, Hyprland 0.48, Weston 15 and later) | sRGB: the setting is accepted, the image is drawn sRGB and the diagnostics say why | the window's surface declared wide: a colour space on the swapchain (eframe and egui-wgpu configure 8-bit sRGB only), or an image surface of its own tagged Rec.2020 + gamma 2.2 through the protocol |
| Wayland without the protocol, X11 | sRGB | nothing: the compositor cannot be told, the image stays sRGB |
| no GPU path (the viewport on the CPU) | sRGB | the CPU path draws 8-bit sRGB textures |
| macOS, Windows | out of scope | their own paths (an extended-range colour space with Metal, HDR swapchains on DX12); the transform already encodes extended sRGB and scRGB |

So today the setting changes nothing on screen anywhere: it states the
wish, and the diagnostics say what stands in the way on this machine.
Turning it on is safe: whatever the window cannot carry falls back to the
sRGB it shows now.

## What is missing, and where

On egui-pop's side (and upstream of it):

- **No surface colour space is Rec.2020 with gamma 2.2.** egui-pop's
  `ColorSpace` mirrors wgpu's colour spaces, which have none for it, so
  `DisplayTarget::is_supported` refuses that target and
  `DisplayTarget::effective` turns it into sRGB before the stage is built.
  The Wayland protocol does name it (`bt2020` + `gamma22`); a surface
  tagged through the protocol would need egui-pop to accept a target
  declared that way (`TargetSource::Tagged`) without a wgpu colour space.
- **Nothing tags a surface on Wayland.** eframe configures every window
  8-bit sRGB; `gpu::install_for_surface` and `gpu::choose_surface` serve
  a host that configures its own surface, which Helios (on eframe) does
  not. winit binds no colour-management object and egui-wgpu takes no
  colour space; egui-pop's upstream register tracks both.
- **The output's preferred description** (its gamut and luminances) is
  not read: the probe lists what the compositor supports, not what the
  screen is.

The research behind the choice of paths (the swapchain colour space
through Mesa, or a tagged subsurface) is in
[wayland-colour.md](wayland-colour.md).
