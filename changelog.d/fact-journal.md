kind: internal

Helios's fact bus is now the journal and relay of the application's bus: each typed fact kind is a journaled event point whose named listeners hear its facts in order, delivered off the publisher's thread with the bus's deadlines; the kinds that cross from a node to the desktop are relayable, read with a codec that accepts fields of other node versions.
