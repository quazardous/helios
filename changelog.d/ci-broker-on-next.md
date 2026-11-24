kind: internal

CI: a push to `next` runs the CI broker's selection of tests instead of the whole suite, and its job summary says how many of the tests ran and why. The whole suite runs nightly (skipped when `next` has not moved), on a manual run, and on a push whose commits carry the trailer `CI: full`; only such a full run promotes `next` to `main` and releases (`cargo xtask promote --run`). Recent failures are read over the last 7 days of the history, not the last 20 runs, plus `next`'s latest runs.
