kind: internal

`cargo xtask ci-broker select` prints the CI broker's always-run set for a change as a nextest filter: the tests near the changed files (helios-gui's `it` modules mapped from its sources), the tests the diff adds or edits, those that failed or flaked in the last 20 runs, and the smoke list `ci/smoke.txt`; `--explain` says why one test runs. CI does not use it yet.
