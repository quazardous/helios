kind: changed

Colours an export's colour space cannot hold (for instance the most
saturated reds and greens of a wide-gamut edit, exported to sRGB) now keep
their hue and lightness and lose only saturation, instead of each channel
being clipped on its own, which shifted hues. Colours the space can hold
are written unchanged.
