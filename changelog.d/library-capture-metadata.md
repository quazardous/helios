kind: changed

The search field's dates now mean the day a photo was taken, by the camera's clock, instead of the day its file was last modified, and they are filtered in the catalogue in one query. Capture time (with its time zone offset when the camera wrote one), camera, lens, focal length, aperture, shutter, ISO, size, orientation and file type are kept for every photo at import; a background job fills them for photos imported earlier, and resumes where it stopped.
