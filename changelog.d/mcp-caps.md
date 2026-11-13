kind: added

`helios-mcp` caps what agents send and get: a request is at most 1 MiB and a `limit` at most 1,000 items, refused past that with a clear error. An answer's lists and text are cut to 1,000 items and 8 MiB with a "truncated, N more" note, never silently. Text from photos' metadata or files (titles, tags, camera strings, paths) is marked in the answer as untrusted data, never instructions.
