kind: internal

`cargo xtask bench-remote` refuses a job whose branch would default to `main` (sent from the main checkout by mistake): it says to run from the worktree, or to pass `branch=main` when main is meant. `cargo xtask bench-remote cancel <ext_job_id>` stops one of one's own jobs on the box.
