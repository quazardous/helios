kind: added

Colour chart calibration measures raw shots: `calibrate.chart` (the `helios_calibrate_chart` tool, `helios control calibrate-chart` and the desktop command) now reads a raw chart shot on its own sensor data, before the camera's input profile, and reports each patch, the light and the correction found. Patches clipped on the sensor are left out, and a chart placed off the image is refused with its reason.
