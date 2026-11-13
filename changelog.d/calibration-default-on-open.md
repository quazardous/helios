kind: added
release: minor

Colour chart calibration works end to end: a raw opened for the first time gets its camera's default profile (the status bar says which, or why none fits); the chart profile module's header in the Parameters panel names the profile, camera and light and switches profiles in one undoable step; "Develop one image" (version 3) carries the module; `calibrate.profiles.export` and `.import` share a profile as a `.helios-chartprofile.json` file, checked on import.
