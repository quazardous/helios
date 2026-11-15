kind: internal

Every job on the bench box shares one workspace check per branch head instead of queueing a per-crate check of its own: a test manifest of several jobs on one branch waits for one check, not one per job.
