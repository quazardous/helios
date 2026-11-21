kind: internal

`cargo xtask bench-remote`: a job is followed by the id its own submit line names and its result read from the box's `status <id>`, so a run can no longer report another job's log and exit. `--push` sends nothing for egui-pop's jobs (they build `egui_pop=`'s branch, not the Helios checkout), and each `--push` submit pushes its own tree (`helios-local-<checkout>-<submit>`), so a run that ends or is replaced never removes the tree a newer run of the same worktree still needs.
