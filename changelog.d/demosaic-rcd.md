kind: added

Raw files from Bayer sensors are now demosaiced with RCD, darktable's
default method, ported from darktable: fine detail and edges come out as
darktable renders them, without the colour fringes of the previous
bilinear method (still available as `method: bilinear`). X-Trans sensors
keep the bilinear method until Markesteijn is ported.
