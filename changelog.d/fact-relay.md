kind: internal

A node's relayable facts reach the desktop through its inspect endpoint's subscribe stream, resumed from a cursor; the desktop takes each one in on its application bus, where the listeners of its point hear it, and resyncs when the node restarted or its journal no longer holds what was missed.
