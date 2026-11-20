kind: changed

The circle, ellipse and gradient mask handles go through helios-rack, on egui_hud 0.11's handle kinds (a point, a radius, a feather, an angle, a band across a line), each with its own maths; a path's handles keep theirs behind egui_hud's code hook. Dragging, Shift, Ctrl, the wheel, a double click and Escape behave as before, one undo step per drag. A handle's hover card and edit area now cover the same 24-point target the drag does.
