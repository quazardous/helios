kind: added

Agents and scripts can bypass a node and bring it back (the MCP tool
`helios_node_bypass`, `helios control bypass NODE on|off`): the node keeps
its parameters and the image renders as if it were absent. A bypassed node
is saved with the image's edit, and a template can place a node bypassed.
