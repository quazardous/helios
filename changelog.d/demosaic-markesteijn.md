kind: added

Raw files from Fujifilm X-Trans sensors are now demosaiced with
Markesteijn's method, darktable's default for them, ported from
darktable: Helios's render of an X-Trans file now matches darktable's to
within a ten-thousandth of a Delta E. Blown highlights are clipped a
touch lower, at darktable's level, so that clipped areas match too.
