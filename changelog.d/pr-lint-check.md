kind: internal

Our own pull requests run one GitHub check, `pr-lint`: every linter of `cargo xtask lint`, `cargo doc` without a warning, the UI index, the catalogues and the changelog fragment, in a few minutes and without building the tests. A pull request merges into `next` when its box jobs and this check are green; `next`'s lint job runs the same steps.
