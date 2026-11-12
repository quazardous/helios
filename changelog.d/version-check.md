kind: internal

`cargo xtask check` fails when an in-tree plugin's Helios range or a workspace member's Cargo.lock line does not follow the workspace version, so a branch that lands after a version bump fails its own CI.
