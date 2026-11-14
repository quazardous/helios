kind: internal

helios-mcp: a panic hunt calls every MCP tool with hostile arguments from its schema, and spoils the JSON-RPC envelope, failing on a panic, a hang, a malformed answer or a file left outside the roots. An empty path is refused on a confined server (it named the server's working folder), a sandbox needs a folder, and a message that is not a JSON object gets an invalid-request error instead of no answer.
