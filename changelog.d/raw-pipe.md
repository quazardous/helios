kind: added

A camera raw can now be developed from start to finish: two new modules,
ported from darktable, prepare the sensor data (black level and scale) and
demosaic it (bilinear, Bayer and X-Trans), reading the camera's levels and
filter pattern on their own. A raw goes through to an exported picture.
