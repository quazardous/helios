kind: added

Agents on `helios-mcp` can be confined to folders: `--read-root DIR` (repeatable) and `--write-root DIR` refuse any path a tool is given outside them, before anything reaches a node. Links leading out of a root, `..` and relative paths are refused, and the refusal names the roots granted. Without a root the server works as before.
