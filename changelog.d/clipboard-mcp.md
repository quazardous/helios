kind: added

Agents can read and use the clipboard through MCP: `helios_clipboard`,
`helios_copy` and `helios_paste` copy from and paste into the desktop
holding a workspace (a parameter, the graph, the masks, the library),
each with a dry run, in a slot of the agent's own: an agent never reads
or overwrites what you copied unless you allow it.
