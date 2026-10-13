kind: internal

`cargo xtask check-commits` builds each commit of a series, runs clippy on it and runs only the tests its own diff touches plus the critical set, and reports per commit what it ran and why; `--full` runs every fast test on each commit as before.
