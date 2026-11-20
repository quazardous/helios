kind: changed

Opening a raw develops its first frame sooner: the steps on the full-resolution mosaic (the source copy, rawprepare, white balance, highlights), colour calibration and the first paint's colour conversion run in row bands over every core, as darktable runs its per-pixel loops, with the same pixels bit for bit. The small renders' straight-from-the-mosaic demosaic reads its rows without a per-photosite lookup, and a module's output is no longer copied once more when it is wrapped as an image.
