kind: fixed

A batch of rating or label changes is now saved whole or not at all: a
crash or a failed write no longer leaves a change without its audit entry
or half a batch applied, and several Helios processes can share one vault
without failing as busy or recording a wrong previous value.
