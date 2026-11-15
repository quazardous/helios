kind: fixed

The node's warnings and errors are now written to its log file (`.helios/NAME.node.log`), with their time and source, at the `info` level by default (`HELIOS_LOG_LEVEL` sets another); a command the node refuses is said there too. They were dropped before. The `helios` and `helios-mcp` tools print their warnings on standard error.
