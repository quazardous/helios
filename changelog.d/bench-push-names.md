kind: internal

`cargo xtask bench-remote --push` sends each checkout's code under its own name on the bench box, so worktrees pushing at the same time no longer overwrite each other.
