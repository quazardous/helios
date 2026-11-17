kind: fixed

Commands run through the node's endpoint are checked in one place before they run. A client connecting to the endpoint can no longer run commands as `user`: that origin belongs to the desktop's own calls. A dry run of a desktop window command that takes no `dry_run` is now refused instead of running for real. `midi` and `remote:NAME` are accepted the same way by node and window commands. A refused window command is logged as refused, with its reason, not as run.
