kind: added

A request sent to a node whose answer never came is now reported as "outcome unknown" instead of "cannot reach the node": the connection closed after the request left, so it may have been applied. `helios` exits with code 4, the MCP tools' error starts with "outcome unknown", and a node client answers the call -32008. Read the state before sending it again; never resend a change blindly.
