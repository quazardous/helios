kind: changed

The darkroom decodes the photos next to the one it opens ahead of time: their neighbours in the Library's order are said near on the bus (`helios.photo.shown`, `helios.photo.near`, `helios.photo.hidden`), and a warmer asks the image-state router for their decoded state in the background, never while the shown photo's states compute. Opening a neighbour takes the warmed decode, or joins the one in flight; a photo that leaves is withdrawn.
