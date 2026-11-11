kind: fixed

Cached thumbnails show at once: the cells in view are read from the thumbnail cache on the frame that draws them (on a few threads, within a frame's time), instead of waiting in the reading queue behind the rest of the library, so a library opened again no longer fills its grid one picture after another.
