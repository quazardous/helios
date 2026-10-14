kind: added

Agents can read and move a workspace's places and modes through MCP (helios_context_read, _request, _go and _set_mode), each with a dry run and a revision check: a desktop that has the workspace open moves at once, otherwise a desktop opening the workspace later comes back where the agent left it.
