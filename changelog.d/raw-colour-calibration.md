kind: changed

A raw now gets darktable's colour calibration by default: its white
balance goes to the camera's D65 reference and colour calibration adapts
the colours from the light the camera measured when the shot was taken,
as darktable's scene-referred workflow does. A darktable edit's colour
calibration set to the camera's illuminant now carries over. Edits made
before keep their look, and pictures are unchanged.
