kind: added
release: minor

Chart profiles can now carry a residual table: after the matrix, a hue and saturation table (DNG's layout) corrects what the matrix leaves on the chart. It is kept only when it lowers the Delta E of patches left out of the fit; the Calibration report says whether it was kept, with Delta E without and with it. Profiles with a table export and import; older profiles still load and render as before.
