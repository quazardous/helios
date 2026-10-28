kind: fixed

Colours on the edge of an export's colour space, such as the pure blue of a
loaded sRGB picture, are now written unchanged. The rounding in colour
profiles places them a hair outside the space, and the gamut mapping then
drained part of their saturation: a reloaded sRGB blue exported again came
out a duller, greener blue.
