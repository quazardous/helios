kind: added

A batch running on a node can now be steered while it runs:
`helios control workflow-cancel`, `workflow-retry`, `workflow-prioritize`,
`workflow-pause` and `workflow-resume` (and the matching MCP tools)
cancel or retry some of its images or all of them, move images to the
front or the back of the queue, and pause or resume it; a paused batch
stays paused when the node restarts.
