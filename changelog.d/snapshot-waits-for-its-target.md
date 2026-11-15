kind: fixed

`gui.snapshot` by name waits for its target: asked while the region is not drawn yet (the darkroom entered a frame after the photo opened), the picture is taken from the first frame that draws it, and fails as not shown only after 3 seconds. The first-user journey pictures the image region in one ask.
