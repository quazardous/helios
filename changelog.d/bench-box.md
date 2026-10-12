kind: internal

Helios's long jobs (full build, test suite, survival, check-commits, GPU tests and timings) can run on a shared bench box with `cargo xtask bench-remote`, on a pushed branch or on the working tree; exit 75 means the box could not serve, never a test failure.
