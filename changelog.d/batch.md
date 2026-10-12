kind: added

`helios batch` applies a network to many images and exports each one to a JPEG, PNG or TIFF file, optionally on top of the image's darktable sidecar, with one JSON report line per image. A failed image does not stop the others, existing files are never overwritten, and Ctrl-C leaves no half-written file; it works with release builds.
