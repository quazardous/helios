kind: fixed

Raw files whose sensor has masked photosites (many Canon cameras) now take
their black level from the median of those photosites, as darktable does,
instead of their mean. Shadows were slightly lifted and tinted before; the
raw chain now matches darktable's within a Delta E 2000 of 0.14 on average.
