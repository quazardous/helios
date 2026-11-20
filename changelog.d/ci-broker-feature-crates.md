kind: internal

The CI broker's test selection stops at the app crate for a feature crate: an edit inside `helios-storage-ui` that leaves its public API and bus points alone selects the crate's tests and the helios-gui tests exercising it (155 tests instead of 1167), not all of helios-gui. `ci-broker select --head REF` measures a past commit.
