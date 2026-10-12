kind: added

`helios batch --sidecar --vault FILE` keeps each darktable sidecar it
applies: the image's darktable edit becomes a version marked as imported,
and the sidecar's rating (or reject) is kept, but only when the image has
none yet, so it never overwrites a rating chosen in Helios.
