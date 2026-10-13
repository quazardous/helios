kind: internal

The bench box has a `test-affected` job (the tests a change touches since `base=`), and `cargo xtask bench-remote --affected` runs every job the selection names, in turn.
