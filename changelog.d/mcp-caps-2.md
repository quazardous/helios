kind: added

`helios-mcp` caps waits and batches: a `timeout_ms` over 10 minutes and a batch over 256 steps are refused with a clear error. An image in an answer over 2048 px on its long edge or 5 MiB is left out with a note saying why. A node's endpoints serve at most 64 connections at once and answer one more with "too many connections".
