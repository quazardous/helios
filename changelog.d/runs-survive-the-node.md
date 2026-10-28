kind: added

A batch kept in a vault now survives its node: started again after a
crash, the node finishes the batch without writing any file twice, and a
second node on the same vault takes over the batches of one that died.
Finished batches are removed after 30 days (`system.workflow.keep_days`),
or at once with `helios control workflow-forget`.
