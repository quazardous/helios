kind: changed

What follows the image open in the darkroom (its pinned snapshots, the labels and locks its edit was saved with, the workshop session's photo) is updated once, in a declared order, when an image is opened or closed (`image.opened`, `image.closed` on the bus), instead of being compared at each pass; the workshop session now learns the photo in the same moment, not a frame later. The journal's `image.opened` and `image.closed` say which followers ran (`followed`).
