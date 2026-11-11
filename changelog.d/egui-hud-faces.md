kind: changed

egui_hud separates controls (a value and a behaviour) from faces, a library per value type that draws them: checkbox, switch, push button, lamp; slider, fader, knob, endless knob, scrub field, meter, read-out; menu, segmented buttons, radio list; the curve, colour and text editors. Kits name the faces a control accepts (`"faces": ["fader", "knob"]`), numbers may wrap (the demo's hue, on the endless knob), and the bench picks a control's face.
