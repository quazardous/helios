kind: internal

Before each release the box's `snapshots-refresh` job regenerates every snapshot suite in a workspace build and lists each changed image with the merged pull request that likely caused it; an image nothing explains stops the release, and `cargo xtask snapshots apply` opens the pull request that commits the rest.
