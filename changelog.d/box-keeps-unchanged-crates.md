kind: internal

The bench box no longer rebuilds every workspace crate on each job: a file whose content did not change since the branch's last job gets its old time back, so cargo builds only what a change touches. `check crate=<name>` now lints that crate alone instead of the whole workspace.
