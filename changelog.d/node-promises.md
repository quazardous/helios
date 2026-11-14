kind: added

Promises on the node: a long command can answer at once with a handle to its result to come, kept in the library so it survives a restart. `promises.get`, `promises.list`, `promises.wait` (answers when it settles or at a timeout), `promises.cancel` and `promises.retry`, with the events `promise.progress` and `promise.done`. A promise cut by a crash is resumed or failed `interrupted`, never left running.
