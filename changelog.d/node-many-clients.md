kind: internal

The bench box's `stability` job also runs many clients on one headless node at once (reads, saves, stacks and their undo, imports, cancels, disconnections, bus subscribers), checks the vault against every acknowledged write and reports calls per second, latencies and the node's memory and descriptors; a short four-client variant runs as a normal test.
