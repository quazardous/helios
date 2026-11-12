kind: added

The node lists its commands and runs them by id: `commands.list` gives every control verb with its label, the JSON schema of its params, what it needs and whether it can run here, with the reason when it cannot (a debug-only verb, a vault verb on a node started without `--vault`); `command.run` checks the params against that schema, then runs the verb as its request would, saved under the origin that ran it (`cli`, `user`, `agent:NAME`) with its undo group. Both are served by the batch endpoint in every build; `helios control commands` and `helios control run ID JSON` use them.
