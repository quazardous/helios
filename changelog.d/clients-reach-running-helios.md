kind: changed
release: minor

The `helios` CLI and the agents' MCP server use the running Helios: while the desktop has a library open, `helios import`, `import-darktable`, `stacks`, `batch --sidecar --vault` and an agent's macro step on an image reach the desktop's library instead of opening it a second time. With no Helios running, they start a background node that exits when idle.
