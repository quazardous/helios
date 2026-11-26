kind: added

Agents: `helios.workflow.list` reads a node's queue as the Queue panel shows it, a page at a time (`from`, `limit`), with its summary and each item's state, attempts, place and actions. It is read-only, so an agent allowed only to read can follow the queue, while cancelling, retrying or reordering still needs write access; file names are given only to agents allowed private data. Its arguments are checked before it runs.
