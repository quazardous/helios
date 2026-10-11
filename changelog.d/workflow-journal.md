kind: added

A workflow can be applied to many items over time: each item's progress
through each step is journaled, workers claim items without ever taking one
twice, a choice skips the branches it did not take, failures are retried
after a growing delay, and work held by a worker that died is given back.
