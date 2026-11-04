kind: changed

In the History panel, a snapshot not saved yet keeps its own name, followed
by "(not saved yet)" in italics, and its tooltip says it is kept at the
edit's next save. Agents reading the history get a `pending` flag on each
snapshot instead of a name to parse.
