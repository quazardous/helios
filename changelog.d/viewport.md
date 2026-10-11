kind: added

Image viewport: the Image panel shows a node's output as float data on
the GPU, a proxy of the whole image first, then the exact render of what
is visible once the view is still, in tiles reused when panning; renders
never block the window. Zoom, centre and fit are values (`view/viewport/*`)
moved by the mouse, the `viewport.zoom.*` commands or an agent, and a
parameter changed through `param.set` renders again. The display
transform (to sRGB, clipped) is the last step; `gui.snapshot` of
`viewport.scene` saves the float data before it. `helios-gui --image FILE`
shows a picture or a raw file's preview.
