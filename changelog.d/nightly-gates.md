kind: internal

The nightly workflows run only when there is something new: `windows-tests` skips a night when `next` has not moved since the commit it last tested, and the `ci-history` ingest skips a night when no run of `next` is left to ingest. A small gate job decides in seconds, a skipped night stays green, and any doubt runs the job.
