kind: added

Calibration profiles apply to photos: `calibrate.apply` puts a profile on photos' edits (a new `chartprofile` step right after input colour), or with `auto` the camera's profile nearest the shot's light, refusing with the reason when none fits; `none` turns it off. `calibrate.profiles.setDefault` chooses which profiles a camera's photos may get. On the desktop an apply is one step Ctrl+Z takes back.
