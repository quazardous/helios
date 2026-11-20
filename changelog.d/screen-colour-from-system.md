kind: added

On Wayland with colour management, Helios now reads what each screen shows (sRGB, wide gamut or HDR) from the compositor, at start and when a screen is plugged in, so a window on a plain sRGB screen no longer asks for a wide-gamut image. `helios doctor display` lists each screen's description and what it shows.
