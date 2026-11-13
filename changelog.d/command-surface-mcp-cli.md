kind: added
release: minor

Every command reaches agents and scripts through one pair generated from the registry: `helios commands` lists the node's and the desktop window's commands with their params' schema and, when greyed, why; `helios run ID --param k=v` (or `--json`) runs one as the CLI. `helios-mcp` offers the same as `helios_commands_list` and `helios_command_run`, run as the agent, a greyed command refused with its reason.
