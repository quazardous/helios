kind: added

`helios sandbox new|start|stop|clean` gives a node its own directory,
settings and sockets in one command each, and an agent can do the same from
MCP. A sandbox is never created over a directory holding something else, and
only a directory marked as a sandbox is ever removed.
