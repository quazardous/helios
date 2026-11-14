kind: added

`helios-mcp --tools generated`: one MCP tool per command of the registry (`helios_cmd_<id>`), generated from it, with the command's params schema as its input schema and `readOnlyHint` as an annotation, beside the five compact tools. A call is checked against the schema, then runs as `helios_command_run` would, with the same caps, roots and request key. `full` stays the default.
