kind: changed

The Queue panel shows the workflow level's queues: a connected node's run rows at the selected graph node, with their state, attempts and error, followed live from the node's facts. Retry, Move to top, Move to bottom and Cancel act on the node's run (`helios.workflow.retry`, `helios.workflow.prioritize`, `helios.workflow.cancel`, with `helios.workflow.pause` and `helios.workflow.resume` beside them, the same commands for agents). Long jobs that are not runs (an export, the trash, a backup) show as their promises.
