kind: internal

The box jobs' preflight no longer asks for a parameter used only inside a `${name:+…}` or `${name:-…}` form, so a whole-crate `egui-pop-test` without `test=` is queued.
