kind: added

A process's inspect endpoint pushes its events as they happen
(`subscribe` with a kind prefix), and a client that reconnects picks up
from the last event it saw, told when some were missed.
