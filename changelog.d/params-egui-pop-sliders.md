kind: changed

The Parameters panel's sliders are egui-pop's slider family, built from
each parameter's descriptor: Ctrl drags and steps ten times finer and
Shift ten times coarser, Page Up and Page Down take coarse steps, Home and
End go to the soft range's ends, Escape gives a drag up without a trace,
and Enter types a value or an expression of the current one (`x * 2`).
A logarithmic bar and a picker button follow the module's hints. Every
finished change runs the `slider.change` step, so a plugin can veto or
snap it; a drag is still one undo step and steps in a row one group.
