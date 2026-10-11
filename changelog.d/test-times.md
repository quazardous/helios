kind: internal

`cargo xtask test-times` runs the tests under cargo-nextest, keeps a history of their durations and names the slowest tests and the ones getting slower; nextest warns about a test running over 10 s and stops it at 120 s.
