kind: added

Image viewport: the Image panel shows a node's output on the GPU, a proxy
first, then the exact visible part in tiles reused when panning, never
blocking the window. Zoom and position are values moved by mouse,
`viewport.zoom.*` or an agent, a changed parameter renders again, and
`gui.snapshot` of `viewport.scene` saves the data before the display
transform. `helios-gui --image FILE` shows a picture.
