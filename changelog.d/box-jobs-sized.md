kind: internal

The bench box's test jobs queue less: `test-crate` and `check` reserve 3 threads and 3 GB (their measured peaks are 3.0 and 2.0 GB), so three run side by side; test jobs no longer wait for a chained `check`; `test-crate` takes several test targets or helios-gui modules in one run (`test=import+storage+lib`); and `gui-snapshots` runs under nextest, one process per test, as a shared-machine run, so a full `test=it` no longer exhausts the memory.
