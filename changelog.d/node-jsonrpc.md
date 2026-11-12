kind: added

The node's inspect, control and batch endpoints speak JSON-RPC 2.0: the method is a command's id, errors carry standard and Helios codes, one connection carries several calls and the event notifications of `events.subscribe`, `node.hello` gives the protocol version and the client's permissions, and `$/cancel` stops a wait still running. The older `{"cmd": …}` requests are still answered. See docs/protocol.md.
