kind: internal

The CI broker's shadow week: every push to `next` still runs the full suite, and records beside it which tests `cargo xtask ci-broker select` would have run (the `broker-shadow` artifact). The nightly history ingest counts the misses, the tests that failed outside the selection, and `cargo xtask ci-history report --shadow` prints the selected share, the time it would have saved and each miss with the files its run changed. Nothing is skipped yet.
