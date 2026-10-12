kind: changed

The image viewport runs on egui-pop's viewport. 100 % now means one image
pixel on one screen pixel at any window scale, and the zoom value shows
100 % there. Zoom in and zoom out walk a ladder of whole ratios (50 %,
100 %, 200 %...), and the wheel stops at 100 % when it crosses it. Edits
re-render in tiles, the old image staying on screen until the new tiles
arrive. New: `viewport.compare` shows before and after side by side
(before is the edit as it was opened; hold `\` to see it alone), and the
working-space values of the pixel under the pointer show in a corner.
