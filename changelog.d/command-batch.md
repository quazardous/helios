kind: added

Several commands in one call: `command.batch` on a node's endpoints, `helios_command_batch` in `helios-mcp` (compact mode too) and `helios batch-run` in the CLI. Each step gets its own result in order; the first failed step stops the rest unless `stop_on_error` is false. Not atomic, at most 256 steps, and a step's `request_id` is applied once.
