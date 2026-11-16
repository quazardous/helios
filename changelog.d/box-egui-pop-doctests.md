kind: internal

The bench box's `egui-pop-test` runs the crate's doctests after its tests on a whole-crate run (`doc=0` skips them), and `test=doc` runs them alone, so a broken doc example fails on the box as it does in egui-pop's CI.
