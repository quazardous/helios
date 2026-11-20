kind: added

Settings > Display lists each screen with what the system says it shows (sRGB, wide gamut, HDR or not described) and a correction: choose what the screen really shows, or Auto to follow the system. The correction wins over the system's report, survives a screen being plugged in again and the next start, and a screen not connected stays listed while it has one. Agents correct a screen with `helios.display.screen_override`. HDR is detected and shown only: an HDR screen is drawn as wide gamut for now.
