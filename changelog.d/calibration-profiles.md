kind: added

Colour chart calibration saves profiles: `calibrate.chart` with `save` (`--save` on `helios control calibrate-chart`, the MCP tool, the desktop command) keeps the correction found as a calibration profile in the library, with its camera, light and quality, and answers its id. `calibrate.profiles.list`, `.get` and `.delete` read and remove them; on the desktop a save or a deletion is undone with Ctrl+Z. A raw from the library is measured with its own edit's settings.
