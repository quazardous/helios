kind: added

Agents can hold the places and modes of the desktop that has a workspace
open, with the new `helios_context_hold` and `helios_context_release`
MCP tools: while held, moving to another place or mode is refused with
the agent's reason. `helios_context_read` lists the holds.
